// LIF-442: the web client's per-project read model.
//
// One replica of a project's live issue + page rows, kept warm in module
// scope and reconciled by deltas instead of by refetching the whole list on
// a timer. It replaces the auto-refresh loop for the list surfaces: a route
// asks for its project's model, renders from memory on the very next frame,
// and the model catches up underneath it.
//
// ── The delta discipline (the whole point) ──
//
//  * The ONLY thing that advances `cursor` is applying a response from
//    `/index` or `/changes`. Websocket events are id-only envelopes; they
//    never advance it, because they carry no row.
//  * A seq-bearing event with `seq > cursor` means "there is something above
//    my cursor" → schedule a pull. `seq <= cursor` is a duplicate from the
//    resume boundary → ignore it.
//  * A seq-less event is advisory ("something happened") → schedule a pull.
//  * `sync_required` means the server's replay ring no longer covers our
//    cursor → schedule a pull, which is exactly the backfill it asks for.
//
// Everything therefore funnels into one debounced `/changes?since=cursor`
// loop. Bursts coalesce; duplicates are idempotent (upsert-if-newer);
// gaps are impossible because the cursor only ever moves by applying rows.

import { getProjectChanges, getProjectIndex, type Project } from "../api";
import { getPublicProject } from "../publicScope";
import type { ChangesPage, IssueRow, PageRow, SyncEvent } from "./types";
import { onReadModelAccountChange, onReadModelProjects, readModelAccount, readModelScope } from "./account";
import { loadSnapshot, removeSnapshot, snapshotOf, storeSnapshot } from "./persistence";

/** Coalescing window for pulls. Long enough that a burst of websocket events
 *  from one bulk mutation becomes a single request, short enough that a
 *  single edit still feels immediate. */
const PULL_DEBOUNCE_MS = 150;
const WRITE_BEHIND_MS = 250;

export type ReadModelStatus = "cold" | "loading" | "ready";

export class ProjectReadModel {
  readonly projectId: number;
  /** LIF-471: the registry key this model was created under. A pull only
   *  runs while the client is still in that audience, so a public replica
   *  is never filled from the authenticated API after a scope change (or the
   *  reverse), however late the pull was scheduled. */
  readonly key: string;

  /** Live issues by id. Reassigned (never mutated in place) on every apply
   *  so `$state` sees the change — a plain Map is not deeply reactive. */
  issues = $state<Map<number, IssueRow>>(new Map());
  pages = $state<Map<number, PageRow>>(new Map());

  /** Highest stream position applied. Only `applyIndex`/`applyChanges` move
   *  it. See the delta discipline note at the top of this file. */
  cursor = $state(0);

  status = $state<ReadModelStatus>("cold");
  /** Last bootstrap failure, for a route that wants to surface it. Cleared
   *  by the next successful bootstrap. */
  error = $state("");

  issueList = $derived([...this.issues.values()]);
  pageList = $derived([...this.pages.values()]);

  /** True only on a genuine cold start: nothing to render AND nothing
   *  rendered before. A warm project is never `coldStart`, which is what
   *  keeps navigation between the list, the board and the page tree free of
   *  loading skeletons. */
  coldStart = $derived(this.status !== "ready" && this.issues.size === 0 && this.pages.size === 0);

  // Non-reactive plumbing.
  private pullTimer: ReturnType<typeof setTimeout> | null = null;
  private pullRun: Promise<void> | null = null;
  private pullQueued = false;
  private bootstrapping: Promise<void> | null = null;
  /** An event arrived while the snapshot was still in flight. `/index`
   *  reads its cursor before its rows, so the snapshot may predate that
   *  write; pull once the bootstrap settles rather than dropping it. */
  private pullAfterBootstrap = false;
  private writeTimer: ReturnType<typeof setTimeout> | null = null;
  private epoch: string | null = null;
  private databaseId = "";
  private skipCache = false;
  private disposed = false;

  constructor(projectId: number, key: string = modelKey(projectId)) {
    this.projectId = projectId;
    this.key = key;
  }

  /** False once the client has crossed the public/private boundary since
   *  this model was created; see `key`. */
  private inScope(): boolean {
    return !this.disposed && modelKey(this.projectId) === this.key;
  }

  dispose(): void {
    this.disposed = true;
    if (models.get(this.key) === this) models.delete(this.key);
    if (this.pullTimer) clearTimeout(this.pullTimer);
    if (this.writeTimer) clearTimeout(this.writeTimer);
    this.pullTimer = this.writeTimer = null;
    this.issues = new Map();
    this.pages = new Map();
    this.cursor = 0;
    this.status = "cold";
  }

  private persist(): void {
    if (!this.inScope() || this.epoch === null || this.writeTimer) return;
    this.writeTimer = setTimeout(() => {
      this.writeTimer = null;
      if (!this.inScope() || this.epoch === null) return;
      void storeSnapshot(this.key, snapshotOf(this.databaseId, this.cursor, this.issues, this.pages), this.epoch);
    }, WRITE_BEHIND_MS);
  }

  /** Cold start. Idempotent: concurrent callers share one request, and a
   *  model that is already `ready` just catches up instead. */
  bootstrap(): Promise<void> {
    if (this.bootstrapping) return this.bootstrapping;
    if (!this.inScope()) return Promise.resolve();
    if (this.status === "ready") {
      this.schedulePull();
      return Promise.resolve();
    }
    this.status = "loading";
    this.bootstrapping = (async () => {
      // An unverified private session can cold-load but cannot read or write
      // another account's disk cache. Public views have their own signed-out scope.
      if (getPublicProject() !== null || readModelAccount() !== undefined) {
        const cached = await loadSnapshot(this.key);
        if (!this.inScope()) return;
        this.epoch = cached.epoch;
        if (cached.snapshot && !this.skipCache) {
          this.issues = new Map(cached.snapshot.issues.map(row => [row.id, row]));
          this.pages = new Map(cached.snapshot.pages.map(row => [row.id, row]));
          this.cursor = cached.snapshot.cursor;
          this.databaseId = cached.snapshot.database_id;
          this.error = "";
          this.status = "ready";
          this.pullAfterBootstrap = true;
          return;
        }
      }
      this.skipCache = false;
      const res = await getProjectIndex(this.projectId);
      if (!this.inScope()) return;
      if (res.ok) {
        this.databaseId = res.data.database_id;
        const issues = new Map<number, IssueRow>();
        for (const row of res.data.issues) issues.set(row.id, row);
        const pages = new Map<number, PageRow>();
        for (const row of res.data.pages) pages.set(row.id, row);
        this.issues = issues;
        this.pages = pages;
        // The snapshot's cursor was read BEFORE its rows, so anything that
        // raced the bootstrap is re-delivered by the first pull rather than
        // silently skipped. Never move a cursor backwards.
        if (res.data.cursor > this.cursor) this.cursor = res.data.cursor;
        this.error = "";
        this.status = "ready";
        this.persist();
        this.pullAfterBootstrap = true;
      } else {
        this.error = res.error;
        this.status = "cold";
      }
    })().finally(() => {
      this.bootstrapping = null;
      if (this.pullAfterBootstrap) {
        this.pullAfterBootstrap = false;
        if (this.status === "ready") this.schedulePull();
      }
    });
    return this.bootstrapping;
  }

  /** Queue a catch-up pull, coalescing anything scheduled in the same
   *  ~150ms window into one request. */
  schedulePull(): void {
    if (!this.inScope()) return;
    if (this.bootstrapping) {
      // The snapshot in flight will land a cursor; pull from there.
      this.pullAfterBootstrap = true;
      return;
    }
    if (this.status !== "ready") {
      void this.bootstrap();
      return;
    }
    if (this.pullTimer) return;
    this.pullTimer = setTimeout(() => {
      this.pullTimer = null;
      void this.pull();
    }, PULL_DEBOUNCE_MS);
  }

  /** Pull immediately, skipping the debounce. Used after a local mutation
   *  whose result the view needs reconciled now: the returned promise
   *  settles only after a `/changes` request that started after this call
   *  has been applied, so a write the caller already saw acknowledged is in
   *  the replica by then. */
  async pull(): Promise<void> {
    if (this.bootstrapping) {
      // Never race a snapshot with a delta: the snapshot would land second
      // and could overwrite newer rows.
      this.pullAfterBootstrap = true;
      await this.bootstrapping;
      return;
    }
    if (this.status !== "ready") {
      await this.bootstrap();
      return;
    }
    if (this.pullRun) {
      // The running pull may have read its page before the caller's write.
      // Queue another round; the run loops until the queue is drained.
      this.pullQueued = true;
      while (this.pullRun) await this.pullRun;
      return;
    }
    if (!this.inScope()) return;
    this.pullRun = this.drain();
    await this.pullRun;
  }

  private async drain(): Promise<void> {
    try {
      do {
        this.pullQueued = false;
        // Loop while the server says there is more above our (just advanced)
        // cursor, so one call drains a large backlog.
        for (;;) {
          if (!this.inScope()) return;
          const res = await getProjectChanges(this.projectId, this.cursor);
          if (!this.inScope()) return;
          if (!res.ok) {
            // Revoked access must not leave cached private rows on screen.
            if (res.status === 401 || res.status === 403 || res.status === 404) {
              this.dispose();
              this.error = res.error;
              void removeSnapshot(this.key);
            }
            return;
          }
          if (res.data.reset || res.data.database_id !== this.databaseId) {
            this.skipCache = true;
            if (this.writeTimer) clearTimeout(this.writeTimer);
            this.writeTimer = null;
            this.issues = new Map();
            this.pages = new Map();
            this.cursor = 0;
            this.databaseId = "";
            this.epoch = null;
            this.status = "loading";
            await removeSnapshot(this.key);
            if (this.inScope()) await this.bootstrap();
            return;
          }
          this.applyChanges(res.data);
          if (!res.data.has_more) break;
        }
      } while (this.pullQueued);
    } finally {
      this.pullQueued = false;
      this.pullRun = null;
    }
  }

  /** Apply one `/changes` page: upsert live rows (replacing only when the
   *  incoming row is newer), drop tombstoned ids, then adopt the response
   *  cursor. Comment rows ride the same stream and are skipped — the
   *  detail views own comments. */
  applyChanges(page: ChangesPage): void {
    if (!this.inScope()) return;
    let issues = this.issues;
    let pages = this.pages;
    let issuesDirty = false;
    let pagesDirty = false;

    for (const change of page.changes) {
      if (change.kind === "issue") {
        if (!issuesDirty) {
          issues = new Map(issues);
          issuesDirty = true;
        }
        if (change.deleted) {
          issues.delete(change.id);
        } else {
          const prev = issues.get(change.id);
          if (!prev || change.seq > prev.seq) issues.set(change.id, change);
        }
      } else if (change.kind === "page") {
        if (!pagesDirty) {
          pages = new Map(pages);
          pagesDirty = true;
        }
        if (change.deleted) {
          pages.delete(change.id);
        } else {
          const prev = pages.get(change.id);
          if (!prev || change.seq > prev.seq) pages.set(change.id, change);
        }
      }
      // kind === "comment": not part of this replica.
    }

    if (issuesDirty) this.issues = issues;
    if (pagesDirty) this.pages = pages;
    if (page.cursor > this.cursor) this.cursor = page.cursor;
    this.persist();
  }

  /** Handle one realtime envelope. Returns whether it scheduled a pull, so
   *  callers can reason about (and test) the duplicate-suppression path. */
  handleEvent(event: SyncEvent): boolean {
    if (this.status !== "ready") {
      // Bootstrapping (or failed): remember that something happened so the
      // snapshot is followed by a catch-up instead of standing alone.
      this.schedulePull();
      return false;
    }
    if (typeof event.seq === "number" && event.seq <= this.cursor) {
      // A duplicate from the resume replay boundary — already applied.
      return false;
    }
    this.schedulePull();
    return true;
  }
}

// ── Registry ────────────────────────────────────────────────
//
// Models for visited projects stay warm for the lifetime of the tab. A
// project's replica is a few hundred KB at most and re-entering it is the
// single most common navigation in the app, so there is no eviction policy:
// the whole point is that the second visit costs nothing.

// LIF-471: keyed by audience as well as project. The public view of a
// project is fed by the anonymous surface and the private view by the
// authenticated one; the rows happen to match today, but a replica must
// never be filled from one and served through the other.
const models = new Map<string, ProjectReadModel>();

function modelKey(projectId: number): string {
  return `${readModelScope()}${projectId}`;
}

/** The project whose websocket resume frame we send on reconnect. Set by
 *  whichever list route resolved its project most recently. */
let activeProjectId: number | null = null;

export function getProjectModel(projectId: number): ProjectReadModel {
  const key = modelKey(projectId);
  let model = models.get(key);
  if (!model) {
    model = new ProjectReadModel(projectId, key);
    models.set(key, model);
  }
  return model;
}

/** The model for a project only if one already exists. Unlike
 *  `getProjectModel`, this never registers a replica, so a lookup cannot
 *  make a later realtime event bootstrap a project nobody opened. */
export function peekProjectModel(projectId: number): ProjectReadModel | null {
  return models.get(modelKey(projectId)) ?? null;
}

/** Get a project's model, bootstrapping it on first access and catching it
 *  up on every subsequent one. The returned model is renderable immediately
 *  — warm on a revisit, empty-with-status-`loading` on a cold start. */
export function ensureProjectModel(projectId: number): ProjectReadModel {
  const model = getProjectModel(projectId);
  if (model.status === "cold") void model.bootstrap();
  else model.schedulePull();
  installGlobalListeners();
  return model;
}

export function setActiveProject(projectId: number | null): void {
  activeProjectId = projectId;
}

/** Force an immediate reconcile for one project — used by routes right
 *  after a local mutation that they could not apply optimistically. */
export function refreshProjectModel(projectId: number | null | undefined): void {
  if (projectId == null) return;
  const model = models.get(modelKey(projectId));
  if (model) void model.pull();
}

// ── Realtime bridge ─────────────────────────────────────────

type FrameSender = (frame: unknown) => void;

let sendFrame: FrameSender | null = null;

/** Called by App.svelte when the websocket opens. Sends a resume frame for
 *  the active project so the server replays what we missed, and schedules a
 *  pull anyway: belt and braces, one cheap request that closes the window
 *  where the replay ring had already rolled past our cursor. */
export function realtimeOpened(send: FrameSender): void {
  sendFrame = send;
  if (activeProjectId == null) return;
  const model = models.get(modelKey(activeProjectId));
  if (!model) return;
  if (model.status === "ready") {
    send({ type: "resume", project_id: model.projectId, cursor: model.cursor });
  }
  model.schedulePull();
}

export function realtimeClosed(): void {
  sendFrame = null;
}

export function hasRealtimeSender(): boolean {
  return sendFrame !== null;
}

/** Route one realtime envelope into the read models. Called by App.svelte
 *  for every event, before it re-broadcasts the DOM event the views that
 *  have not been converted still listen to. */
export function handleRealtimeEvent(event: SyncEvent): void {
  // A dropped-and-restored connection invalidates every replica's
  // assumption that it saw the intervening events.
  if (event.type === "resync.required") {
    for (const model of models.values()) {
      if (model.key.startsWith("private:")) model.schedulePull();
    }
    return;
  }

  const projectId = typeof event.project_id === "number" ? event.project_id : null;
  if (projectId == null) return;
  // Realtime is a private channel; it only ever feeds private replicas.
  if (getPublicProject() !== null) return;
  const model = models.get(modelKey(projectId));
  if (!model) return;

  // `sync_required` is the server saying "my replay ring no longer covers
  // your cursor, backfill over HTTP" — which is precisely a pull.
  if (event.type === "sync_required") {
    model.schedulePull();
    return;
  }

  model.handleEvent(event);
}

// ── Tab focus ───────────────────────────────────────────────
//
// Replaces the focus-revalidate half of the old auto-refresh loop for the
// converted views. One debounced pull for the active project when the tab
// comes back; the visibilitychange + focus pair that both fire on a
// tab-switch-back collapse into a single request via schedulePull.

let listenersInstalled = false;

function pullActiveOnFocus(): void {
  if (typeof document !== "undefined" && document.hidden) return;
  if (activeProjectId == null) return;
  models.get(modelKey(activeProjectId))?.schedulePull();
}

function installGlobalListeners(): void {
  if (listenersInstalled) return;
  if (typeof document === "undefined" || typeof window === "undefined") return;
  listenersInstalled = true;
  document.addEventListener("visibilitychange", pullActiveOnFocus);
  window.addEventListener("focus", pullActiveOnFocus);
}

// ── Project resolution cache ────────────────────────────────
//
// The list routes resolve a project by identifier via `listProjects()`.
// That round trip is what made a warm navigation still flash a skeleton:
// the read model had the rows the whole time, but the route had no project
// id to ask for them with yet. Caching the resolved projects lets a revisit
// pick the project synchronously and render from memory on the first frame,
// with the fetch still running underneath to pick up renames and new
// projects.

const projectsByIdentifier = new Map<string, Project>();

function projectKey(identifier: string): string {
  const audience = modelKey(0);
  return `${audience}:${identifier.toLowerCase()}`;
}

export function cacheProjects(projects: Project[]): void {
  for (const project of projects) {
    projectsByIdentifier.set(projectKey(project.identifier), project);
  }
}

export function cachedProject(identifier: string): Project | null {
  return projectsByIdentifier.get(projectKey(identifier)) ?? null;
}

onReadModelAccountChange(() => {
  for (const model of models.values()) model.dispose();
  models.clear();
  projectsByIdentifier.clear();
  activeProjectId = null;
});

onReadModelProjects((scope, projects) => {
  const allowed = new Set(projects);
  for (const model of models.values()) {
    if (model.key.startsWith(scope) && !allowed.has(model.projectId)) model.dispose();
  }
  for (const [key, project] of projectsByIdentifier) {
    if (key.startsWith(scope) && !allowed.has(project.id)) projectsByIdentifier.delete(key);
  }
});

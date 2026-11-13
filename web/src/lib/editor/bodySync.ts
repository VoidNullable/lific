// Keeps an open body editor and the server's copy of that body in step.
//
// The editor saves on its own a moment after typing stops, and on blur. Saves
// carry the `expected_seq` the editor last synced with, so a body changed
// elsewhere (an agent's `edit_issue`, another tab) is never overwritten
// blindly. Bodies changed elsewhere reach the editor as text; they are turned
// into edits and applied as a transaction, so the caret, the selection and
// the undo history survive.
//
// Local and remote edits that touch different parts of the body are merged:
// the remote edits are mapped over what the person typed since the last sync
// and the other way round, the way collaborative editors rebase. When both
// sides changed the same stretch of text nothing is merged; the editor reports
// a conflict and keeps the person's text until they pick a version.
//
// The class owns no DOM. The host gives it the current text, a way to apply
// changes to the editor, and the save call; tests drive it with a plain
// string standing in for the editor.

import { ChangeSet } from "@codemirror/state";
import { diffText } from "./textDiff";

export type BodySaveResult =
  | { ok: true; body: string; seq: number | null }
  /** The server holds a newer body; `current` is what it holds now. */
  | { ok: false; conflict: { body: string; seq: number | null } }
  | { ok: false; error: string };

export type SyncStatus =
  /** Everything typed is on the server. */
  | "saved"
  /** Typed but not yet sent (waiting for a pause). */
  | "pending"
  | "saving"
  /** The last save failed; it is retried on its own. */
  | "error"
  /** Someone else changed the same text; the person has to choose. */
  | "conflict";

export interface BodySyncHost {
  /** The editor's current text. */
  doc(): string;
  /** Apply changes to the editor without reporting them back as local edits.
   *  `undoable` is true when the person asked for it (taking the other
   *  version), so Ctrl+Z can bring their text back. */
  apply(changes: ChangeSet, undoable: boolean): void;
  save(text: string, expectedSeq: number | null): Promise<BodySaveResult>;
  onStatus?(status: SyncStatus, detail?: string): void;
  /** Remote edits were merged into text the person had not saved yet. */
  onMerged?(): void;
  setTimer?(fn: () => void, ms: number): unknown;
  clearTimer?(handle: unknown): void;
}

export interface BodySyncOptions {
  /** Save after this long without typing. */
  idleMs?: number;
  /** Save at least this often during continuous typing. */
  maxWaitMs?: number;
}

const RETRY_MS = [2_000, 5_000, 15_000, 30_000];

/** The edits that turn `from` into `to`, as a change set over `from`. */
export function changeSetBetween(from: string, to: string): ChangeSet {
  return ChangeSet.of(diffText(from, to), from.length);
}

function changedRanges(changes: ChangeSet): [number, number][] {
  const out: [number, number][] = [];
  changes.iterChangedRanges((fromA, toA) => out.push([fromA, toA]));
  return out;
}

/** Whether two change sets over the same text touch the same stretch. Edits
 *  that merely meet at a point count, since the order of two insertions at
 *  one spot is a guess. */
export function changesOverlap(a: ChangeSet, b: ChangeSet): boolean {
  const ra = changedRanges(a);
  const rb = changedRanges(b);
  for (const [af, at] of ra) {
    for (const [bf, bt] of rb) {
      if (af <= bt && bf <= at) return true;
    }
  }
  return false;
}

export class BodySync {
  /** The server's text as of the last sync. */
  private base: string;
  private seq: number | null;
  /** A save on the wire: the text sent and the edits from `base` to it. */
  private inflight: { text: string; changes: ChangeSet } | null = null;
  /** Edits from `inflight.text` (or `base` when idle) to the editor's text. */
  private unsent: ChangeSet;
  private queuedRemote: { text: string; seq: number | null } | null = null;
  private theirs: { text: string; seq: number | null } | null = null;
  private status: SyncStatus = "saved";
  private idleTimer: unknown = null;
  private maxTimer: unknown = null;
  private retryTimer: unknown = null;
  private failures = 0;
  private destroyed = false;
  private readonly idleMs: number;
  private readonly maxWaitMs: number;

  constructor(
    private readonly host: BodySyncHost,
    text: string,
    seq: number | null,
    opts: BodySyncOptions = {},
  ) {
    this.base = text;
    this.seq = seq;
    this.unsent = ChangeSet.empty(text.length);
    this.idleMs = opts.idleMs ?? 1_500;
    this.maxWaitMs = opts.maxWaitMs ?? 10_000;
  }

  /** True while anything typed has not reached the server. */
  get dirty(): boolean {
    return !this.unsent.empty || this.inflight !== null || this.theirs !== null;
  }

  get currentStatus(): SyncStatus {
    return this.status;
  }

  /** The other version while in conflict. */
  get conflictText(): string | null {
    return this.theirs?.text ?? null;
  }

  /** Report an edit the person made in the editor. */
  local(changes: ChangeSet): void {
    if (changes.empty || this.destroyed) return;
    this.unsent = this.unsent.compose(changes);
    if (this.theirs) return;
    if (this.unsent.empty && !this.inflight) {
      this.setStatus("saved");
      return;
    }
    this.setStatus(this.inflight ? "saving" : "pending");
    this.schedule();
  }

  /** The server's body changed (a refresh, a realtime event). */
  remote(text: string, seq: number | null): void {
    if (this.destroyed) return;
    if (this.theirs) {
      if (this.host.doc() === text) {
        this.adopt(text, seq);
        this.theirs = null;
        this.setStatus("saved");
      } else {
        this.theirs = { text, seq };
        this.setStatus("conflict");
      }
      return;
    }
    if (this.inflight) {
      this.queuedRemote = { text, seq };
      return;
    }
    if (seq !== null && this.seq !== null && seq <= this.seq) return;
    this.merge(text, seq);
  }

  /** Send what is unsaved now instead of waiting for a pause. */
  async flush(): Promise<void> {
    this.clearSchedule();
    if (this.destroyed || this.inflight || this.theirs) return;
    if (this.unsent.empty) {
      if (this.status !== "error") this.setStatus("saved");
      return;
    }
    const text = this.host.doc();
    this.inflight = { text, changes: this.unsent };
    this.unsent = ChangeSet.empty(text.length);
    this.setStatus("saving");
    let result: BodySaveResult;
    try {
      result = await this.host.save(text, this.seq);
    } catch (e) {
      result = { ok: false, error: e instanceof Error ? e.message : String(e) };
    }
    const sent = this.inflight;
    this.inflight = null;
    if (this.destroyed || !sent) return;

    if (result.ok) {
      this.failures = 0;
      this.base = text;
      this.seq = result.seq ?? this.seq;
      if (result.body !== text) this.merge(result.body, result.seq, true);
    } else {
      // Put the sent edits back in front of anything typed since.
      this.unsent = sent.changes.compose(this.unsent);
      if ("conflict" in result) {
        this.failures = 0;
        this.merge(result.conflict.body, result.conflict.seq, true);
      } else {
        this.failures += 1;
        this.setStatus("error", result.error);
        this.scheduleRetry();
      }
    }

    const queued = this.queuedRemote;
    this.queuedRemote = null;
    if (queued) this.remote(queued.text, queued.seq);

    if (this.theirs || this.status === "error") return;
    if (!this.unsent.empty) {
      // A conflict that merged cleanly still has the person's edits to send;
      // send them now rather than after another pause.
      if (!result.ok) void this.flush();
      else {
        this.setStatus("pending");
        this.schedule();
      }
    } else {
      this.setStatus("saved");
    }
  }

  /** Settle a conflict. "mine" saves the editor's text over the other
   *  version; "theirs" replaces the editor's text with it (undoable). */
  resolveConflict(choice: "mine" | "theirs"): void {
    const theirs = this.theirs;
    if (!theirs) return;
    this.theirs = null;
    const doc = this.host.doc();
    if (choice === "theirs") {
      this.host.apply(changeSetBetween(doc, theirs.text), true);
      this.adopt(theirs.text, theirs.seq);
      this.setStatus("saved");
      return;
    }
    this.base = theirs.text;
    this.seq = theirs.seq ?? this.seq;
    this.unsent = changeSetBetween(theirs.text, doc);
    void this.flush();
  }

  destroy(): void {
    this.destroyed = true;
    this.clearSchedule();
    if (this.retryTimer !== null) this.clear(this.retryTimer);
    this.retryTimer = null;
  }

  // ── internals ──────────────────────────────────────────

  /** Bring a newer server text in. `force` skips the stale-seq check for
   *  answers to our own save. */
  private merge(text: string, seq: number | null, force = false): void {
    if (!force && seq !== null && this.seq !== null && seq <= this.seq) return;
    if (text === this.base) {
      this.seq = seq ?? this.seq;
      return;
    }
    const remote = changeSetBetween(this.base, text);
    const local = this.unsent;
    if (local.empty) {
      this.host.apply(remote, false);
      this.adopt(text, seq);
      return;
    }
    if (this.host.doc() === text) {
      // Both sides already say the same thing.
      this.adopt(text, seq);
      return;
    }
    if (changesOverlap(local, remote)) {
      this.theirs = { text, seq };
      this.clearSchedule();
      this.setStatus("conflict");
      return;
    }
    this.host.apply(remote.map(local), false);
    this.unsent = local.map(remote, true);
    this.base = text;
    this.seq = seq ?? this.seq;
    this.host.onMerged?.();
  }

  private adopt(text: string, seq: number | null) {
    this.base = text;
    this.seq = seq ?? this.seq;
    this.unsent = ChangeSet.empty(text.length);
  }

  private setStatus(status: SyncStatus, detail?: string) {
    if (status === this.status && status !== "error") return;
    this.status = status;
    this.host.onStatus?.(status, detail);
  }

  private set(fn: () => void, ms: number): unknown {
    return this.host.setTimer ? this.host.setTimer(fn, ms) : setTimeout(fn, ms);
  }

  private clear(handle: unknown) {
    if (this.host.clearTimer) this.host.clearTimer(handle);
    else clearTimeout(handle as ReturnType<typeof setTimeout>);
  }

  private schedule() {
    if (this.idleTimer !== null) this.clear(this.idleTimer);
    this.idleTimer = this.set(() => void this.flush(), this.idleMs);
    if (this.maxTimer === null) this.maxTimer = this.set(() => void this.flush(), this.maxWaitMs);
  }

  private scheduleRetry() {
    if (this.retryTimer !== null) this.clear(this.retryTimer);
    const delay = RETRY_MS[Math.min(this.failures - 1, RETRY_MS.length - 1)];
    this.retryTimer = this.set(() => {
      this.retryTimer = null;
      void this.flush();
    }, delay);
  }

  private clearSchedule() {
    if (this.idleTimer !== null) this.clear(this.idleTimer);
    if (this.maxTimer !== null) this.clear(this.maxTimer);
    this.idleTimer = null;
    this.maxTimer = null;
  }
}

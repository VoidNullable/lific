import type { IndexSnapshot, IssueRow, PageRow } from "./types";

const DATABASE = "lific-read-model";
const VERSION = 1;
const SNAPSHOTS = "snapshots";
const META = "meta";

export interface StoredSnapshot extends IndexSnapshot {
  version: number;
}

function record(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function integer(value: unknown): value is number {
  return Number.isSafeInteger(value) && (value as number) >= 0;
}

function nullable(value: unknown, type: "number" | "string"): boolean {
  return value === null || (typeof value === type && (type !== "number" || integer(value)));
}

function row(value: unknown, kind: "issue" | "page"): boolean {
  if (!record(value) || value.kind !== kind || value.deleted !== false ||
      !integer(value.id) || !integer(value.seq)) return false;
  if (!["identifier", "title", "status", "created_at", "updated_at", "preview"]
    .every(key => typeof value[key] === "string")) return false;
  if (!Array.isArray(value.labels) || !value.labels.every(label => typeof label === "string")) return false;
  if (kind === "page") return nullable(value.folder_id, "number") && typeof value.pinned === "boolean";
  if (typeof value.priority !== "string" || !Number.isFinite(value.sort_order) ||
      !nullable(value.module_id, "number") || !nullable(value.start_date, "string") ||
      !nullable(value.target_date, "string")) return false;
  if (value.needs_human !== undefined && typeof value.needs_human !== "boolean") return false;
  if (value.waits !== undefined && (!Array.isArray(value.waits) || !value.waits.every(wait =>
    record(wait) && integer(wait.id) && integer(wait.issue_id) &&
    typeof wait.note === "string" && typeof wait.created_at === "string" &&
    ["holding", "due", "overdue"].includes(wait.state as string) &&
    (wait.kind === "user"
      ? integer(wait.user_id) && typeof wait.username === "string"
      : wait.kind === "date" && typeof wait.earliest === "string" && typeof wait.latest === "string")))) return false;
  if (value.assignees !== undefined && (!Array.isArray(value.assignees) || !value.assignees.every(assignee =>
    record(assignee) && integer(assignee.user_id) && typeof assignee.username === "string" &&
    (assignee.display_name === undefined || typeof assignee.display_name === "string")))) return false;
  return true;
}

/** Disk is an optional cache, never a trusted wire response. */
export function validSnapshot(value: unknown): value is StoredSnapshot {
  if (!record(value) || value.version !== VERSION || !integer(value.cursor) ||
      typeof value.database_id !== "string" || !value.database_id) return false;
  return (["issues", "pages"] as const).every((key) => {
    const rows = value[key];
    return Array.isArray(rows) && rows.every(item => row(item, key === "issues" ? "issue" : "page")) &&
      new Set(rows.map(item => item.id)).size === rows.length;
  });
}

function open(): Promise<IDBDatabase | null> {
  return new Promise(resolve => {
    if (typeof indexedDB === "undefined") return resolve(null);
    let request: IDBOpenDBRequest;
    try { request = indexedDB.open(DATABASE, VERSION); } catch { return resolve(null); }
    let settled = false;
    const finish = (db: IDBDatabase | null) => {
      if (settled) { db?.close(); return; }
      settled = true;
      clearTimeout(timer);
      resolve(db);
    };
    // A blocked upgrade or unavailable storage must not block the network path.
    const timer = setTimeout(() => finish(null), 1000);
    request.onblocked = () => finish(null);
    request.onerror = () => finish(null);
    request.onupgradeneeded = () => {
      const db = request.result;
      for (const name of [...db.objectStoreNames]) db.deleteObjectStore(name);
      db.createObjectStore(SNAPSHOTS);
      db.createObjectStore(META);
    };
    request.onsuccess = () => {
      const db = request.result;
      db.onversionchange = () => db.close();
      finish(db);
    };
  });
}

export interface CacheRead {
  snapshot: StoredSnapshot | null;
  epoch: string | null;
}

function generation(): string {
  return [...crypto.getRandomValues(new Uint32Array(4))].join("-");
}

function privateAccountKey(key: string): boolean {
  return /^private:\d+:/.test(key);
}

function guard(tx: IDBTransaction, key: string) {
  const meta = tx.objectStore(META);
  const global = meta.get("epoch");
  const privateEpoch = meta.get("private-epoch");
  const local = meta.get(`epoch:${key}`);
  const scope = key.slice(0, key.lastIndexOf(":") + 1);
  const scopeEpoch = meta.get(`epoch-scope:${scope}`);
  const projects = meta.get(`projects:${scope}`);
  return {
    ready: projects,
    epoch: () => JSON.stringify([global.result ?? "", privateAccountKey(key) ? privateEpoch.result ?? "" : "", local.result ?? "", scopeEpoch.result ?? ""]),
    allowed: () => !Array.isArray(projects.result) || projects.result.includes(Number(key.slice(scope.length))),
  };
}

export async function loadSnapshot(key: string): Promise<CacheRead> {
  const empty = { snapshot: null, epoch: null };
  const db = await open();
  if (!db) return empty;
  return new Promise(resolve => {
    try {
      const tx = db.transaction([SNAPSHOTS, META], "readonly");
      const snapshot = tx.objectStore(SNAPSHOTS).get(key);
      const fence = guard(tx, key);
      tx.oncomplete = () => {
        db.close();
        resolve({
          snapshot: fence.allowed() && validSnapshot(snapshot.result) ? snapshot.result : null,
          epoch: fence.allowed() ? fence.epoch() : null,
        });
      };
      tx.onabort = tx.onerror = () => { db.close(); resolve(empty); };
    } catch { db.close(); resolve(empty); }
  });
}

/** A whole replica is one record: cursor, upserts and removals commit together.
 *  The epoch fences writes queued by any tab before sign-out. */
export async function storeSnapshot(key: string, snapshot: IndexSnapshot, epoch: string): Promise<boolean> {
  const db = await open();
  if (!db) return false;
  return new Promise(resolve => {
    try {
      const tx = db.transaction([SNAPSHOTS, META], "readwrite");
      const store = tx.objectStore(SNAPSHOTS);
      let written = false;
      const fence = guard(tx, key);
      fence.ready.onsuccess = () => {
        try {
          if (fence.epoch() !== epoch || !fence.allowed()) return;
          const previous = store.get(key);
          previous.onsuccess = () => {
            try {
              // A slower tab cannot rewind the durable cursor.
              if (validSnapshot(previous.result) && previous.result.database_id === snapshot.database_id &&
                  previous.result.cursor > snapshot.cursor) return;
              store.put({ ...snapshot, version: VERSION }, key);
              written = true;
            } catch { tx.abort(); }
          };
        } catch { tx.abort(); }
      };
      tx.oncomplete = () => { db.close(); resolve(written); };
      tx.onabort = tx.onerror = () => { db.close(); resolve(false); };
    } catch { db.close(); resolve(false); }
  });
}

export async function clearSnapshots(): Promise<void> {
  const db = await open();
  if (!db) return;
  return new Promise(resolve => {
    try {
      const tx = db.transaction([SNAPSHOTS, META], "readwrite");
      tx.oncomplete = tx.onabort = tx.onerror = () => { db.close(); resolve(); };
      try {
        tx.objectStore(SNAPSHOTS).clear();
        tx.objectStore(META).put(generation(), "epoch");
      } catch { tx.abort(); }
    } catch { db.close(); resolve(); }
  });
}

export async function removeSnapshot(key: string): Promise<void> {
  const db = await open();
  if (!db) return;
  return new Promise(resolve => {
    try {
      const tx = db.transaction([SNAPSHOTS, META], "readwrite");
      tx.oncomplete = tx.onabort = tx.onerror = () => { db.close(); resolve(); };
      try {
        tx.objectStore(SNAPSHOTS).delete(key);
        tx.objectStore(META).put(generation(), `epoch:${key}`);
      } catch { tx.abort(); }
    } catch { db.close(); resolve(); }
  });
}

async function prune(
  remove: (key: string) => boolean,
  fenceKey: string,
  fenceValue: unknown,
): Promise<void> {
  const db = await open();
  if (!db) return;
  return new Promise(resolve => {
    try {
      const tx = db.transaction([SNAPSHOTS, META], "readwrite");
      tx.oncomplete = tx.onabort = tx.onerror = () => { db.close(); resolve(); };
      try {
        tx.objectStore(META).put(fenceValue, fenceKey);
        const cursor = tx.objectStore(SNAPSHOTS).openCursor();
        cursor.onsuccess = () => {
          try {
            const entry = cursor.result;
            if (!entry) return;
            if (typeof entry.key === "string" && remove(entry.key)) entry.delete();
            entry.continue();
          } catch { tx.abort(); }
        };
      } catch { tx.abort(); }
    } catch { db.close(); resolve(); }
  });
}

/** Anonymous readers retain public caches, never another person's private rows. */
export function clearPrivateSnapshots(): Promise<void> {
  if (typeof indexedDB === "undefined") return Promise.resolve();
  return prune(privateAccountKey, "private-epoch", generation());
}

/** The allowlist also rejects queued writes for projects already removed. */
export function pruneProjectSnapshots(scope: string, projects: number[]): Promise<void> {
  const allowed = new Set(projects);
  return prune(key => key.startsWith(scope) && !allowed.has(Number(key.slice(scope.length))),
    `projects:${scope}`, projects);
}

export function removeScopeSnapshots(scope: string): Promise<void> {
  if (typeof indexedDB === "undefined") return Promise.resolve();
  return prune(key => key.startsWith(scope), `epoch-scope:${scope}`, generation());
}

/** Copy the arrays while the in-memory cursor still describes exactly these rows. */
export function snapshotOf(database_id: string, cursor: number, issues: Map<number, IssueRow>, pages: Map<number, PageRow>): IndexSnapshot {
  return { database_id, cursor, issues: [...issues.values()], pages: [...pages.values()] };
}

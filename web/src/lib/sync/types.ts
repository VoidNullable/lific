// LIF-442: wire types for the delta-sync read surface (server: LIF-439).
//
// These mirror `src/db/models.rs`'s `IssueChange` / `PageChange` /
// `CommentChange` / `Tombstone` exactly. Every row is *skinny*: identity,
// its position in the project's sync stream (`seq`), and the fields a list
// or board renders. Full descriptions, page content and comment bodies are
// deliberately absent — a detail view fetches those. Issue and page rows do
// carry a bounded `preview`, which is what a list row actually renders.
//
// `seq` is a per-project monotonic stream position. A client's `cursor` is
// the highest seq it has applied; `/changes?since=cursor` returns everything
// above it, tombstones included.

import type { IssueWait } from "../api";

export type SyncKind = "issue" | "page" | "comment";

/** A live issue row. `labels` carries label NAMES (not ids), matching the
 *  `Issue.labels` shape the list views already filter on. */
export interface IssueRow {
  kind: "issue";
  seq: number;
  /** Always false — a deleted issue arrives as a {@link Tombstone}. */
  deleted: false;
  id: number;
  identifier: string;
  title: string;
  status: string;
  priority: string;
  module_id: number | null;
  sort_order: number;
  start_date: string | null;
  target_date: string | null;
  created_at: string;
  updated_at: string;
  /** First non-empty line of the description, trimmed and capped at 200
   *  characters. `""` when the issue has no description. NOT the document:
   *  a detail view still fetches the full body. */
  preview: string;
  labels: string[];
  /** LIF-484: user and date blockers. Omitted when the issue has none; a
   *  wait added or cleared re-delivers the row at a new seq. */
  waits?: IssueWait[];
}

/** A live page row. Note the field the wire shape does NOT carry: no
 *  `content` (only its `preview`) and no `sort_order`. */
export interface PageRow {
  kind: "page";
  seq: number;
  deleted: false;
  id: number;
  identifier: string;
  title: string;
  status: string;
  folder_id: number | null;
  pinned: boolean;
  created_at: string;
  updated_at: string;
  /** First non-empty line of the content, trimmed and capped at 200
   *  characters. `""` when the page is empty. */
  preview: string;
  /** LIF-105 page label NAMES, project-scoped. */
  labels: string[];
}

/** A live comment row. The read model tracks issues and pages only, so this
 *  exists to be recognized and skipped rather than mistaken for a row shape
 *  we store. */
export interface CommentRow {
  kind: "comment";
  seq: number;
  deleted: false;
  id: number;
  issue_id: number | null;
  page_id: number | null;
  user_id: number;
  username: string;
  created_at: string;
  updated_at: string;
}

/** A deleted row: identity + stream position, nothing else. `kind` still
 *  names the table it came from, so the replica knows which map to drop it
 *  from. */
export interface Tombstone {
  kind: SyncKind;
  seq: number;
  deleted: true;
  id: number;
}

export type Change = IssueRow | PageRow | CommentRow | Tombstone;

/** `GET /api/projects/{id}/index` — the cold-start snapshot. Live rows only;
 *  `cursor` is read before the lists, so a write racing the bootstrap is
 *  re-delivered by the first `/changes` pull rather than skipped. */
export interface IndexSnapshot {
  cursor: number;
  issues: IssueRow[];
  pages: PageRow[];
}

/** `GET /api/projects/{id}/changes?since=N` — ascending by seq. */
export interface ChangesPage {
  changes: Change[];
  cursor: number;
  has_more: boolean;
}

/** A realtime envelope as it arrives on the websocket. Events are id-only;
 *  `seq` is present on the ones the server buffers for replay and absent on
 *  advisory ones. */
export interface SyncEvent {
  type: string;
  project_id?: unknown;
  seq?: unknown;
  [key: string]: unknown;
}

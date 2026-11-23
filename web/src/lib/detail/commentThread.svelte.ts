// The comment thread of one detail route (an issue or a page): the window of
// newest comments on screen, paging back through older ones, and the local
// fold after a create, edit or delete. IssueDetail and PageDetail each carried
// a copy of this; the ordering rules live in `commentState.ts` and are
// explained there.
//
// Two counters order the work. The route generation (owned by the route, read
// through `gen()`) says whether a result still belongs to the document on
// screen. The operation counter orders the reads that replace or extend the
// window within one route: last started wins. A mutation does not claim it,
// it bumps the epoch instead, so a replacement that read the thread before
// the edit refuses to land and reads again.

import { untrack } from "svelte";
import type { Comment, CommentCursor, CommentPage, RequestResult } from "../api";
import {
  COMMENT_WINDOW_RETRY_LIMIT,
  commentOpIsCurrent,
  commentWindowOutcome,
  loadCommentWindow,
  olderCursor,
  prependOlderComments,
  reconcileCommentWindow,
  removeComment,
  upsertComment,
  type CommentWindow,
} from "../commentState";
import { toast } from "../toast/toast.svelte";

export interface CommentThreadApi {
  list: (parentId: number, before: CommentCursor | null, size?: number) => Promise<RequestResult<CommentPage>>;
  create: (parentId: number, content: string) => Promise<RequestResult<Comment>>;
  update: (id: number, content: string) => Promise<RequestResult<Comment>>;
  remove: (id: number) => Promise<RequestResult<unknown>>;
  /** The route generation; bumped by the route on every navigation. */
  gen: () => number;
  /** The id of the document on screen, or null while none is loaded. */
  currentId: () => number | null;
  /** Whether a write for `parentId` may still fold into the thread. */
  isCurrent: (parentId: number) => boolean;
  /** After a successful create, edit or delete (refresh activity, say). */
  onMutated?: (parentId: number) => void;
}

export class CommentThread {
  comments = $state<Comment[]>([]);
  hasOlder = $state(false);
  loadingOlder = $state(false);

  #op = 0;
  #epoch = 0;
  #api: CommentThreadApi;

  constructor(api: CommentThreadApi) {
    this.#api = api;
  }

  /** Take the window for a new replacing operation, cancelling the last. */
  #claim(): number {
    this.loadingOlder = false;
    this.#op += 1;
    return this.#op;
  }

  #merge(fresh: CommentWindow) {
    const merged = reconcileCommentWindow({ items: this.comments, hasOlder: this.hasOlder }, fresh);
    this.comments = merged.items;
    this.hasOlder = merged.hasOlder;
  }

  /** Empty the thread for a new document. Anything in flight is cancelled. */
  reset() {
    this.#claim();
    this.comments = [];
    this.hasOlder = false;
  }

  /** Read the newest window for `parentId`. A background refresh re-reads as
   *  many rows as are on screen, so edits further up the thread show too. */
  async load(parentId: number, gen: number, background = false): Promise<void> {
    const token = { route: gen, op: this.#claim(), epoch: this.#epoch };
    // Untracked: a route effect may call this synchronously, and reading the
    // list it is about to replace would make that effect depend on it.
    const rows = background ? untrack(() => this.comments.length) : 0;
    const res = await loadCommentWindow((before, size) => this.#api.list(parentId, before, size), rows);
    if (!res.ok) return;
    const outcome = commentWindowOutcome(token, this.#api.gen(), this.#op, this.#epoch);
    if (outcome === "apply") this.#merge(res.data);
    else if (outcome === "retry") void this.#restabilize(parentId, gen);
  }

  /** A mutation landed while a replacement was in flight: read again against
   *  the new epoch, a bounded number of times. */
  async #restabilize(parentId: number, gen: number) {
    for (let attempt = 0; attempt < COMMENT_WINDOW_RETRY_LIMIT; attempt += 1) {
      if (gen !== this.#api.gen() || !this.#api.isCurrent(parentId)) return;
      const token = { route: gen, op: this.#claim(), epoch: this.#epoch };
      const res = await loadCommentWindow(
        (before, size) => this.#api.list(parentId, before, size),
        this.comments.length,
      );
      if (!res.ok) return;
      const outcome = commentWindowOutcome(token, this.#api.gen(), this.#op, this.#epoch);
      if (outcome === "abandon") return;
      if (outcome === "retry") continue;
      this.#merge(res.data);
      return;
    }
  }

  async loadOlder(): Promise<void> {
    const parentId = this.#api.currentId();
    if (parentId === null || this.loadingOlder || !this.hasOlder) return;
    const gen = this.#api.gen();
    const op = this.#claim();
    this.loadingOlder = true;
    // Keyed on the oldest comment on screen, so a comment posted meanwhile
    // cannot shift what "the previous page" means.
    const res = await this.#api.list(parentId, olderCursor(this.comments));
    if (!commentOpIsCurrent({ route: gen, op }, this.#api.gen(), this.#op)) return;
    this.loadingOlder = false;
    if (!res.ok) {
      toast(`Couldn't load older comments: ${res.error}`, { kind: "error" });
      return;
    }
    this.comments = prependOlderComments(this.comments, res.data.items);
    this.hasOlder = res.data.hasMore;
  }

  // Each mutation remembers the document it was for. On success the epoch is
  // bumped and the result folded in, so the user sees it at once; the folds
  // are idempotent, so the same row arriving later cannot duplicate.
  #fold(parentId: number, apply: () => void) {
    if (!this.#api.isCurrent(parentId)) return;
    this.#epoch += 1;
    apply();
    this.#api.onMutated?.(parentId);
  }

  async create(content: string): Promise<Comment | null> {
    const parentId = this.#api.currentId();
    if (parentId === null) return null;
    const res = await this.#api.create(parentId, content);
    if (!res.ok) {
      toast(`Couldn't add comment: ${res.error}`, { kind: "error" });
      return null;
    }
    this.#fold(parentId, () => (this.comments = upsertComment(this.comments, res.data)));
    return res.data;
  }

  async update(id: number, content: string): Promise<Comment | null> {
    const parentId = this.#api.currentId();
    if (parentId === null) return null;
    const res = await this.#api.update(id, content);
    if (!res.ok) {
      toast(`Couldn't update comment: ${res.error}`, { kind: "error" });
      return null;
    }
    this.#fold(parentId, () => (this.comments = upsertComment(this.comments, res.data)));
    return res.data;
  }

  async remove(id: number): Promise<boolean> {
    const parentId = this.#api.currentId();
    if (parentId === null) return false;
    const res = await this.#api.remove(id);
    if (!res.ok) {
      toast(`Couldn't delete comment: ${res.error}`, { kind: "error" });
      return false;
    }
    this.#fold(parentId, () => (this.comments = removeComment(this.comments, id)));
    return true;
  }
}

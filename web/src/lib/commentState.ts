import {
  COMMENT_PAGE_SIZE,
  commentCursor,
  type Comment,
  type CommentCursor,
  type CommentPage,
  type RequestResult,
} from "./api";

export type CommentKeyboardContext = "new" | "edit" | "menu";
export type CommentKeyboardAction = "submit" | "save" | "cancel" | "close-menu";

export function commentKeyboardAction(
  context: CommentKeyboardContext,
  key: string,
  modified: boolean,
): CommentKeyboardAction | null {
  if (key === "Enter" && modified) {
    if (context === "new") return "submit";
    if (context === "edit") return "save";
  }
  if (key === "Escape") {
    if (context === "edit") return "cancel";
    if (context === "menu") return "close-menu";
  }
  return null;
}

export function canManageComment(
  comment: Comment,
  currentUser: { id: number } | null,
  actionsAvailable: boolean,
): boolean {
  return actionsAvailable && currentUser?.id === comment.user_id;
}

export function commentWasEdited(comment: Comment): boolean {
  return comment.updated_at !== comment.created_at;
}

/** Canonical thread order: `(created_at, id)`.
 *
 *  The same key the keyset cursor pages by, deliberately. If the list on
 *  screen ordered itself any other way, the row a cursor is derived from would
 *  not be the row the server considers the boundary, and paging would start
 *  skipping or repeating comments. `created_at` has one-second resolution, so
 *  the id half decides ties, which are common rather than exotic. */
export function compareComments(a: Comment, b: Comment): number {
  if (a.created_at !== b.created_at) return a.created_at < b.created_at ? -1 : 1;
  return a.id - b.id;
}

/** Fold one comment into the thread by id, keeping canonical order.
 *
 *  Every local mutation goes through this, and it has to be idempotent because
 *  the same comment can legitimately arrive twice: folded in here after a
 *  write, and again from a refresh that was already in flight. Blindly
 *  appending would put the same id in the list twice, which Svelte's keyed
 *  `{#each}` turns into a runtime error rather than a visual glitch.
 *
 *  A present comment is replaced where it stands. Neither `created_at` nor
 *  `id` changes on an edit, so its position is already correct and re-sorting
 *  could only move it wrongly. An absent one is placed by the comparator
 *  rather than appended: usually that is the end, because a newly created
 *  comment is the newest, but a comment that is not in the loaded window
 *  belongs in the middle and appending it would put it visibly out of order
 *  and hand the next cursor the wrong boundary. The input is never mutated. */
export function upsertComment(comments: Comment[], comment: Comment): Comment[] {
  const index = comments.findIndex((existing) => existing.id === comment.id);
  if (index >= 0) {
    const next = [...comments];
    next[index] = comment;
    return next;
  }
  const at = comments.findIndex((existing) => compareComments(comment, existing) < 0);
  if (at < 0) return [...comments, comment];
  return [...comments.slice(0, at), comment, ...comments.slice(at)];
}

/** Drop a comment by id. A no-op when it is already gone, so a delete applied
 *  twice cannot fail or resurrect anything. */
export function removeComment(comments: Comment[], id: number): Comment[] {
  return comments.filter((comment) => comment.id !== id);
}

// ── Bounded, stable comment paging ──────────────────────────
//
// The thread on screen is always a contiguous run ending at the newest
// comment. Older pages are fetched backwards with a keyset cursor rather than
// an offset, because a thread is written to while it is read: an offset points
// at a different row the moment anyone posts above it, which shows up as a
// duplicated comment (and a duplicate Svelte key) or a silently skipped one.

/** Fetch `pageSize` comments immediately before `before`, or the newest page
 *  when it is null.
 *
 *  The size is a parameter rather than the fetcher's own business because the
 *  budget below is expressed in rows on the wire, and it can only hold if the
 *  caller controls how many each request asks for. Injected so the window
 *  logic is testable without a network and shared by both routes. */
export type CommentPageFetcher = (
  before: CommentCursor | null,
  pageSize: number,
) => Promise<RequestResult<CommentPage>>;

/** Prepend an older page, dropping any id already on screen.
 *
 *  The cursor makes overlap impossible in theory. The dedupe is here anyway
 *  because the cost of being wrong is a duplicate keyed `{#each}` row, which
 *  Svelte turns into a hard runtime error rather than a cosmetic glitch. */
export function prependOlderComments(existing: Comment[], older: Comment[]): Comment[] {
  const seen = new Set(existing.map((comment) => comment.id));
  return [...older.filter((comment) => !seen.has(comment.id)), ...existing];
}

/** The contiguous run of newest comments currently held by a route. */
export interface CommentWindow {
  /** Chronological, oldest loaded first. */
  items: Comment[];
  /** Whether comments exist before the oldest loaded one. */
  hasOlder: boolean;
}

/** The cursor that loads the page before a window, or null when it is empty. */
export function olderCursor(comments: Comment[]): CommentCursor | null {
  return comments.length > 0 ? commentCursor(comments[0]) : null;
}

/** Most requests one automatic refresh may issue, whatever it is asked for. */
export const COMMENT_REFRESH_PAGE_BUDGET = 10;

/** Most comment rows one automatic refresh may pull over the wire in total.
 *
 *  This counts rows *transferred*, not rows kept. Each request asks for one
 *  row past its page so the response can answer `hasMore`, and that row is
 *  paid for whether or not it is displayed. Budgeting the retained rows
 *  instead would quietly under-count every request by one and let a full
 *  refresh exceed its own ceiling. */
export const COMMENT_REFRESH_TRANSFER_LIMIT = 500;

/** Force a caller-supplied count into `1..=bound`.
 *
 *  The limits below are the difference between a background refresh and an
 *  accidental crawl, so they are not left to the caller to get right. Anything
 *  that is not a positive finite number, or is larger than the bound, becomes
 *  the bound; fractions round down. `0`, `-1` and `NaN` therefore yield a
 *  working default rather than a loop that never runs or never stops. */
function boundedCount(value: number, bound: number): number {
  const whole = Math.floor(value);
  return Number.isFinite(whole) && whole > 0 ? Math.min(whole, bound) : bound;
}

/** Reload the newest `minRows` comments as a fresh contiguous window.
 *
 *  A background refresh that only re-fetched the newest page would leave every
 *  older page the reader had loaded frozen at the moment it arrived: a comment
 *  edited or deleted by someone else up there would never change on screen.
 *  So the refresh walks back over as much as is already loaded, one bounded
 *  keyset page at a time.
 *
 *  Three hard limits hold no matter what any caller passes: at most
 *  {@link COMMENT_REFRESH_PAGE_BUDGET} requests, at most
 *  {@link COMMENT_PAGE_SIZE} rows asked for per request (so every request stays
 *  well inside the server's own 500-row cap), and at most
 *  {@link COMMENT_REFRESH_TRANSFER_LIMIT} rows transferred across the whole
 *  refresh, lookahead rows included. `minRows` cannot lift any of them.
 *  Without that the cost of an automatic refresh would be whatever history the
 *  reader had manually accumulated: page back far enough and every focus event
 *  fires dozens of requests. At the default page size that works out to nine
 *  requests, 459 rows transferred and at most 450 retained. Rows past the
 *  ceiling are not abandoned, they are simply not *automatically* re-read; see
 *  {@link reconcileCommentWindow}. */
export async function loadCommentWindow(
  fetchPage: CommentPageFetcher,
  minRows = 0,
  pageSize = COMMENT_PAGE_SIZE,
  pageBudget = COMMENT_REFRESH_PAGE_BUDGET,
): Promise<RequestResult<CommentWindow>> {
  const size = boundedCount(pageSize, COMMENT_PAGE_SIZE);
  const budget = boundedCount(pageBudget, COMMENT_REFRESH_PAGE_BUDGET);
  const rows = Number.isFinite(minRows) && minRows > 0 ? Math.floor(minRows) : 0;
  // A request costs `size + 1` rows on the wire, not `size`: the extra row is
  // the lookahead that answers `hasMore`. One page is always allowed, since
  // `size + 1` is at most 51 and cannot breach the limit on its own.
  const affordable = Math.floor(COMMENT_REFRESH_TRANSFER_LIMIT / (size + 1));
  const maxPages = Math.max(1, Math.min(budget, affordable));
  const target = Math.min(Math.max(rows, size), maxPages * size);
  let items: Comment[] = [];
  let hasOlder = false;
  let cursor: CommentCursor | null = null;

  for (let page = 0; page < maxPages; page += 1) {
    const res = await fetchPage(cursor, size);
    if (!res.ok) return res;
    items = prependOlderComments(items, res.data.items);
    hasOlder = res.data.hasMore;
    cursor = res.data.nextCursor;
    if (!hasOlder || items.length >= target || res.data.items.length === 0) break;
  }
  // Pages come in whole, so the last one overshoots whenever `target` is not a
  // multiple of the page size: refreshing a 51-row window would hand back 100
  // rows and quietly grow the thread on every refresh. Trim back to exactly
  // what was asked for, keeping the newest rows, and say that older ones
  // exist, because the rows just dropped are precisely that.
  if (items.length > target) {
    items = items.slice(items.length - target);
    hasOlder = true;
  }
  return { ok: true, data: { items, hasOlder } };
}

/** Fold a freshly refreshed window into the one on screen.
 *
 *  A refresh is bounded (see {@link loadCommentWindow}), but a reader who
 *  pressed "Load older" enough times can hold more than it covers. Replacing
 *  the window outright would throw their history away on the next focus event;
 *  keeping both without care would duplicate rows or leave a hole. The
 *  refreshed window always runs from its oldest row to the newest comment, so
 *  everything on screen strictly older than that row is below it, contiguous
 *  with it, and survives.
 *
 *  The tradeoff, deliberate: preserved rows are not re-read, so an edit or
 *  deletion made elsewhere to a comment more than 500 rows back stays invisible
 *  until the reader revisits it. That is the price of a refresh whose cost does
 *  not grow with how much history someone has scrolled through, and it is the
 *  right side to err on for a background operation.
 *
 *  `hasOlder` comes from whichever window still owns the oldest row: if
 *  anything was preserved, the oldest loaded comment did not move, so what lies
 *  below it did not change either. */
export function reconcileCommentWindow(
  existing: CommentWindow,
  refreshed: CommentWindow,
): CommentWindow {
  const boundary = refreshed.items[0];
  // An empty refresh means an empty thread: nothing survives it.
  if (boundary === undefined) return refreshed;
  const preserved = existing.items.filter(
    (comment) =>
      comment.created_at < boundary.created_at ||
      (comment.created_at === boundary.created_at && comment.id < boundary.id),
  );
  if (preserved.length === 0) return refreshed;
  return {
    items: prependOlderComments(refreshed.items, preserved),
    hasOlder: existing.hasOlder,
  };
}

/** The route and comment-window generation an async operation started under. */
export interface CommentOpToken {
  route: number;
  op: number;
}

/** Whether an operation that started under `token` may still write comment
 *  state.
 *
 *  Two counters, because they answer different questions. The route generation
 *  says "is this even the issue the reader is looking at". The operation
 *  generation orders the things that all legitimately belong to *one* route and
 *  all replace or extend the same list: the initial window, a background
 *  refresh, a manual "load older", and the local fold after a create, edit or
 *  delete. Without it a refresh and a manual page racing on the same issue
 *  interleave, and whichever returns last wins by accident.
 *
 *  The rule is last-started-wins: every one of those operations claims a fresh
 *  token before its first await, and only the holder of the newest token may
 *  land. That also makes the spinner unambiguous, since the operation that
 *  supersedes a manual page is the one that clears its flag. */
export function commentOpIsCurrent(
  token: CommentOpToken,
  route: number,
  op: number,
): boolean {
  return token.route === route && token.op === op;
}

/** Everything a replacement refresh captures when it starts. */
export interface CommentWindowToken extends CommentOpToken {
  epoch: number;
}

/** How many times a replacement will re-read before letting the local folds
 *  stand. Sustained mutation churn should stop the loop, not feed it. */
export const COMMENT_WINDOW_RETRY_LIMIT = 3;

/** What to do with a replacement window that has come back.
 *
 *  `abandon` means a newer operation owns the list; it is already producing a
 *  better answer and this one must not touch anything.
 *
 *  `retry` means a mutation landed while this read was in flight. The read
 *  predates the edit, so applying it would undo work the user watched succeed,
 *  but *discarding* it is equally wrong: on a navigation back to a thread with
 *  a write still pending, the mutation's fold is the only row on screen and
 *  dropping the replacement leaves it as the entire thread. Read again
 *  instead, against the epoch the mutation just established. */
export type CommentWindowOutcome = "apply" | "retry" | "abandon";

export function commentWindowOutcome(
  token: CommentWindowToken,
  route: number,
  op: number,
  epoch: number,
): CommentWindowOutcome {
  if (!commentOpIsCurrent(token, route, op)) return "abandon";
  return token.epoch === epoch ? "apply" : "retry";
}

/** Whether a deep link to a specific comment needs another page loaded first.
 *
 *  A link straight to `#comment-812` on a thread whose newest page starts at
 *  #900 used to resolve to nothing at all: the anchor simply never existed in
 *  the DOM and the view sat on the newest page with no hint that it was the
 *  wrong one. Answering true here drives the older-page callback, and the
 *  caller re-asks after each page lands until the comment shows up or the
 *  thread runs out. */
export function anchorNeedsOlderPage(
  target: string | null,
  comments: Comment[],
  hasOlder: boolean,
  loadingOlder: boolean,
): boolean {
  if (!target || !hasOlder || loadingOlder) return false;
  const match = target.match(/^comment-([1-9]\d*)$/);
  if (!match) return false;
  const id = Number(match[1]);
  return !comments.some((comment) => comment.id === id);
}

/** How many pages a deep link may pull in on its own before it gives up. */
export const ANCHOR_AUTO_PAGE_BUDGET = 5;

/** How far back a deep link searches automatically, in comments. */
export const ANCHOR_AUTO_SEARCH_LIMIT = ANCHOR_AUTO_PAGE_BUDGET * COMMENT_PAGE_SIZE;

/** One anchor-driven walk: which thread and comment it is chasing, how many
 *  comments were on screen when the last request went out, and how much of the
 *  automatic budget it has spent. */
export interface AnchorPageAttempt {
  /** Route-scoped identity of the thread's parent. Two different issues can
   *  hold the same number of comments, and without this a walk abandoned on
   *  one route would silently suppress or resume on the next. */
  parent: string;
  target: string;
  /** Comments on screen when the last automatic request went out. */
  loaded: number;
  /** Automatic pages requested so far for this parent and target. */
  pages: number;
}

/** The next anchor-driven page request to make, or null to stop.
 *
 *  Walking back to a deep-linked comment is a loop driven by state changes, so
 *  it needs reasons to stop that do not depend on the comment ever being
 *  found. There are three, and all of them matter:
 *
 *  - A failed fetch leaves the thread exactly as long as it was, so an attempt
 *    is made at most once per thread length. Re-firing on unchanged state is
 *    how one bad request becomes a request storm.
 *  - A link to a comment that was deleted, or that lives thousands of rows
 *    back, would otherwise walk an entire thread one page at a time on page
 *    load. The budget caps that at {@link ANCHOR_AUTO_PAGE_BUDGET} pages, and
 *    the reader continues by hand from there if they want to.
 *  - Spending the budget is permanent for that walk. A manual "Load older"
 *    still lets the scroll effect find the comment if it turns up, but it does
 *    not hand the automatic search another five pages.
 *
 *  Changing parent or target is a different walk and starts fresh. */
export function nextAnchorAttempt(
  parent: string,
  target: string | null,
  comments: Comment[],
  hasOlder: boolean,
  loadingOlder: boolean,
  last: AnchorPageAttempt | null,
  budget = ANCHOR_AUTO_PAGE_BUDGET,
): AnchorPageAttempt | null {
  if (!anchorNeedsOlderPage(target, comments, hasOlder, loadingOlder)) return null;
  const sameWalk = last !== null && last.parent === parent && last.target === target;
  if (sameWalk && (last.pages >= budget || last.loaded === comments.length)) return null;
  return {
    parent,
    target: target as string,
    loaded: comments.length,
    pages: sameWalk ? last.pages + 1 : 1,
  };
}

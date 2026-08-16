// Page peek panel state — the pages sibling of issues/peek.svelte.ts.
//
// Pages had NO preview surface anywhere in the app (issues have hover cards
// and the peek panel); this powers the first one, opened by press-and-hold
// on page rows (mobile's substitute for hover) and available to any future
// desktop affordance the same way.
//
// Same singleton shape as peekState: one page previewed at a time, panel
// mounted once in Layout.svelte. `href` rides along because the full-view
// route needs the project identifier segment (`/LIF/pages/12`) and the
// caller always has it while the panel would have to derive it.

class PagePeekState {
  open = $state(false);
  pageId = $state<number | null>(null);
  href = $state<string | null>(null);
}

export const pagePeekState = new PagePeekState();

/** Open the peek on a page. Swapping pages while open re-fetches in place
 *  without a close/reopen animation, mirroring openPeek. */
export function openPagePeek(pageId: number, href: string): void {
  pagePeekState.pageId = pageId;
  pagePeekState.href = href;
  pagePeekState.open = true;
}

/** Close the panel. `pageId` stays set so the slide-out keeps its content. */
export function closePagePeek(): void {
  pagePeekState.open = false;
}

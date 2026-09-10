/** Intercept only ordinary activation. The hash URL remains usable by the
 * browser for new tabs, modified clicks, and copying a destination. */
export function navLink(node: HTMLAnchorElement, navigate: (path: string) => void) {
  const click = (event: MouseEvent) => {
    if (event.defaultPrevented || event.button !== 0 || event.metaKey || event.ctrlKey || event.altKey || event.shiftKey) return;
    event.preventDefault();
    navigate(node.hash.slice(1) || "/");
  };
  node.addEventListener("click", click);
  return {
    update(next: (path: string) => void) { navigate = next; },
    destroy() { node.removeEventListener("click", click); },
  };
}

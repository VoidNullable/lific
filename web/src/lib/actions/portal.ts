// Move an element to the end of <body> while it is mounted. For overlays
// rendered from inside a container that would trap `position: fixed`: the
// issue sidebar is a translated, scrolling drawer, so a dialog opened from
// it would otherwise be clipped to the drawer. Svelte delegates events at
// the document as well as the app root, so handlers keep working.
export function portal(node: HTMLElement) {
  document.body.appendChild(node);
  return {
    destroy() {
      node.remove();
    },
  };
}

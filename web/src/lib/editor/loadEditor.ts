// CodeMirror and the editor around it live in their own chunk. Anyone who can
// edit a body starts loading it as soon as the body mounts; read-only visits
// never fetch it.

type EditorModule = typeof import("./cm/index");

let pending: Promise<EditorModule> | null = null;

export function loadEditor(): Promise<EditorModule> {
  pending ??= import("./cm/index").catch((e) => {
    // Let a later mount try again (a flaky network, a deploy mid-session).
    pending = null;
    throw e;
  });
  return pending;
}

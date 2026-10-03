/** Register from a route's mount lifecycle; call the returned cleanup on unmount. */
export interface PaletteRegistration {
  actions?: readonly PaletteAction[];
  results?: readonly PaletteResult[];
  /** Display the route's actual bindings; the route owns their key handlers. */
  shortcuts?: readonly PaletteShortcut[];
}

export interface PaletteShortcut {scope: string; keys: string; label: string;}

export interface PaletteResult {
  kind: 'issue' | 'page' | 'plan' | 'project' | 'module' | 'folder';
  title: string;
  /** Canonical same-origin private route for a project in the current catalog. */
  route: string;
  identifier?: string;
  sub?: string;
}

interface ActionFields {
  id: string;
  title: string;
  hint?: string;
  /** Checked when rendering and again immediately before invoking the action. */
  requires?: 'edit' | 'manage' | 'comment' | 'publish' | 'admin';
}

export type PaletteAction = ActionFields & (
  | {run: () => void; children?: never; prompt?: never}
  | {run?: never; children: () => readonly PaletteChild[]; prompt?: never}
  | {run?: never; children?: never; prompt: {placeholder?: string; initial?: string; submit: (value: string) => void}}
);

export interface PaletteChild {
  title: string;
  hint?: string;
  run: () => void;
}

export interface PaletteRegistry {
  open(): Promise<void>;
  close(): void;
  register(owner: string, registration: PaletteRegistration): () => void;
}

declare global {
  interface Window {lificPalette: PaletteRegistry | null;}
}

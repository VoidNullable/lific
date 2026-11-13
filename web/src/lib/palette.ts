// LIF-159 follow-up: context-aware palette actions.
//
// Routes (via DocumentDetail) register actions through Layout's
// "lific:palette" context, the same pattern the chrome topbar uses.
// Three behaviors:
//   run      — execute immediately and close
//   children — open a submenu (statuses, labels, modules…)
//   prompt   — switch the palette input into a text prompt (rename)

export interface PaletteActionChild {
  title: string;
  /** Shown faintly on the right (e.g. "current"). */
  hint?: string;
  /** Render a StatusIcon for this value. */
  status?: string;
  /** Render a PriorityIcon for this value. */
  priority?: string;
  /** Render a label color dot. */
  color?: string;
  /** Render a project icon (emoji or `lucide:Name`; "" for the default). */
  emoji?: string | null;
  run: () => void;
}

export interface PaletteAction {
  id: string;
  /** "Set status…", "Rename issue", "Add comment" */
  title: string;
  /** Current value shown faintly (e.g. "active"). */
  hint?: string;
  run?: () => void;
  children?: () => PaletteActionChild[];
  prompt?: {
    placeholder?: string;
    initial?: string;
    submit: (value: string) => void;
  };
}

export interface PaletteContext {
  set: (actions: PaletteAction[] | undefined) => void;
}

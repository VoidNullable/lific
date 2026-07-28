<script lang="ts">
  // LIF-109 — segmented mode toggle.
  //
  // Both labels are visible at all times. A pill-shaped indicator slides
  // between the two options to mark which is active. Same control shape
  // appears in two places:
  //
  //   - `size="sm"`         — compact, lives in the page/issue toolbar
  //   - `size="floating"`   — large, fixed to the bottom-right of the
  //                           viewport so the user can flip modes from
  //                           anywhere on a long page
  //
  // The component is presentational only — it doesn't own state or know
  // about the editor. The parent passes `mode` + `onSelect(next)` and
  // is responsible for actually entering/leaving edit mode.

  import { Pencil, Eye } from "lucide-svelte";

  let {
    mode,
    onSelect,
    size = "sm",
    disabled = false,
  }: {
    mode: "read" | "edit";
    onSelect: (next: "read" | "edit") => void;
    size?: "sm" | "floating";
    disabled?: boolean;
  } = $props();

  function pick(next: "read" | "edit") {
    if (disabled) return;
    if (next === mode) return;
    onSelect(next);
  }
</script>

<div
  class="mt"
  class:mt--floating={size === "floating"}
  class:mt--sm={size === "sm"}
  role="radiogroup"
  aria-label="Content view mode"
>
  <!--
    The indicator pill is absolutely positioned in the track. It slides
    via translateX between the left half (Edit) and the right half
    (Preview). transform is GPU-friendly and the easing curve matches
    the rest of the app's motion language (--ease-out-expo).
  -->
  <span
    class="mt__pill"
    aria-hidden="true"
    style:transform={mode === "edit" ? "translateX(0%)" : "translateX(100%)"}
  ></span>

  <button
    type="button"
    class="mt__opt"
    data-active={mode === "edit"}
    role="radio"
    aria-checked={mode === "edit"}
    aria-label="Edit"
    title="Edit (E)"
    {disabled}
    onclick={() => pick("edit")}
  >
    <Pencil size={14} />
    <span class="mt__label">Edit</span>
  </button>

  <button
    type="button"
    class="mt__opt"
    data-active={mode === "read"}
    role="radio"
    aria-checked={mode === "read"}
    aria-label="Preview"
    title="Preview"
    {disabled}
    onclick={() => pick("read")}
  >
    <Eye size={14} />
    <span class="mt__label">Preview</span>
  </button>
</div>

<style>
  /*
   * Track. Two equal-width slots via grid; the sliding pill is laid out
   * on top via absolute positioning. Container gets a subtle inset look
   * so the elevated pill clearly reads as "above" the track.
   */
  .mt {
    position: relative;
    display: inline-grid;
    grid-template-columns: 1fr 1fr;
    align-items: stretch;
    padding: 3px;
    background: var(--bg-subtle);
    border: 1px solid var(--border);
    border-radius: 999px;
    /* Subtle inset shadow grounds the track so the pill on top reads
       as elevated rather than floating in space. */
    box-shadow: inset 0 1px 2px rgb(0 0 0 / 0.04);
  }

  /*
   * The sliding indicator. Width = exactly half of the inner track
   * minus padding, so translateX(100%) lands it neatly in the right
   * slot. We deliberately animate transform-only — color and shadow
   * changes on the *options* run in parallel for a richer feel.
   */
  .mt__pill {
    position: absolute;
    top: 3px;
    bottom: 3px;
    left: 3px;
    width: calc(50% - 3px);
    background: var(--surface);
    border-radius: 999px;
    box-shadow:
      0 1px 2px rgb(0 0 0 / 0.06),
      0 1px 3px rgb(0 0 0 / 0.08);
    transition: transform 0.32s var(--ease-out-expo);
    z-index: 0;
  }

  .mt__opt {
    position: relative;
    z-index: 1;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 0.375rem;
    background: transparent;
    border: 0;
    border-radius: 999px;
    font-family: var(--font-body);
    font-weight: 500;
    color: var(--text-muted);
    cursor: pointer;
    user-select: none;
    transition: color 0.22s var(--ease-out-expo);
  }
  .mt__opt:hover:not([data-active="true"]):not(:disabled) {
    color: var(--text);
  }
  .mt__opt[data-active="true"] {
    color: var(--text);
  }
  .mt__opt:disabled {
    cursor: not-allowed;
    opacity: 0.5;
  }
  .mt__opt:focus-visible {
    outline: none;
    box-shadow: 0 0 0 3px var(--accent-subtle);
  }

  /*
   * Both sizes share the same options dimensions on purpose — the two
   * surfaces mirror each other exactly so the floating one and the
   * toolbar one read as the same control just in different locations.
   * The only difference is the floating variant's positioning + entrance
   * animation; visual contrast (track / border / pill / type) is
   * identical.
   */
  .mt__opt {
    padding: 0.4375rem 1rem;
    font-size: 0.8125rem;
    letter-spacing: 0.005em;
  }

  /*
   * Phones: icon only. At 208px wide with both labels, the toolbar
   * variant was the single largest item in the detail topbar and shoved
   * Export, the delete menu, and the props-panel toggle clean off a
   * 360px screen — measured at 597px of content in a 360px row, with
   * "Preview" itself sliced in half at the viewport edge.
   *
   * Icon-only takes it to ~84px. The accessible name moves to
   * aria-label on each option so hiding the text costs nothing to a
   * screen reader, and the pill + active color still carry the state.
   * Scoped to --sm: the floating variant fixes itself to a corner with
   * a whole viewport to spare and keeps its labels.
   */
  @media (max-width: 639px) {
    .mt--sm .mt__opt {
      padding: 0.4375rem 0.75rem;
    }
    .mt--sm .mt__label {
      display: none;
    }
  }

  /* ── Floating variant ─────────────────────────────────
   *
   * Viewport-fixed bottom-right. Visually mirrors the toolbar variant
   * — same paddings, same colors, same type. Just gets a soft drop
   * shadow so it reads as an element sitting on top of the content
   * rather than embedded in it, and a gentle entrance so it doesn't
   * pop into view abruptly on mount.
   *
   * z-index sits below modal dropdowns (which use z-20 internally) but
   * above ordinary content. If a dropdown ever overlaps, the dropdown
   * should win.
   */
  .mt--floating {
    position: fixed;
    /* LIF-279: clear the home-indicator safe area on notched phones. */
    bottom: max(1.5rem, env(safe-area-inset-bottom));
    right: 1.5rem;
    z-index: 30;
    /* Light single-stop shadow — enough to lift it off the page but
       not so heavy that it visually competes with the toolbar twin. */
    box-shadow:
      0 1px 2px rgb(0 0 0 / 0.04),
      0 4px 14px rgb(0 0 0 / 0.08);
    animation: mt-enter 0.28s var(--ease-out-expo);
  }

  @keyframes mt-enter {
    from {
      opacity: 0;
      transform: translateY(6px);
    }
    to {
      opacity: 1;
      transform: translateY(0);
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .mt__pill,
    .mt__opt {
      transition-duration: 0.001s;
    }
    .mt--floating {
      animation: none;
    }
  }
</style>

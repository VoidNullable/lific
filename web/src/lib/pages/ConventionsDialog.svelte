<script module lang="ts">
  export type ConventionsDialogMode = "template" | "off" | "on";
</script>

<script lang="ts">
  // LIF-524: the deliberate changes to a conventions page, each confirmed
  // with what it does: replace the text with a template, or take the page out
  // of agents' briefings and put it back.
  //
  // A native modal <dialog>: the browser keeps focus inside it and makes the
  // rest of the page inert while it is open.

  import { listConventionTemplates, type ConventionTemplate, type ConventionTemplateKey } from "../api";
  import ConventionsTemplatePicker from "./ConventionsTemplatePicker.svelte";
  import { defaultTemplateKey, templateName } from "./conventions";
  import { X } from "lucide-svelte";
  import { tick } from "svelte";

  let {
    mode = $bindable<ConventionsDialogMode | null>(null),
    pageIdentifier,
    onReplace,
    onSetEnabled,
    returnFocus,
  }: {
    /** Which dialog is open; null when none is. */
    mode?: ConventionsDialogMode | null;
    pageIdentifier: string;
    /** Resolves true once the page holds the template's text. */
    onReplace: (template: ConventionTemplate) => Promise<boolean>;
    onSetEnabled: (enabled: boolean) => Promise<boolean>;
    /** Where focus goes on close when the control that opened the dialog
     *  is gone (a menu item, say). */
    returnFocus?: () => HTMLElement | null;
  } = $props();

  let templates = $state<ConventionTemplate[] | null>(null);
  let templatesError = $state("");
  let selected = $state<ConventionTemplateKey>("empty");
  let busy = $state(false);
  let dialogEl = $state<HTMLDialogElement | null>(null);
  let opener: HTMLElement | null = null;

  const chosen = $derived(templates?.find((t) => t.key === selected) ?? null);

  $effect(() => {
    const el = dialogEl;
    if (!el) return;
    if (mode !== null && !el.open) {
      busy = false;
      const active = document.activeElement;
      opener = active instanceof HTMLElement && active !== document.body ? active : null;
      if (mode === "template" && templates === null) void loadTemplates();
      el.showModal();
      void tick().then(() => el.querySelector<HTMLElement>("[data-autofocus]")?.focus());
    } else if (mode === null && el.open) {
      el.close();
    }
  });

  async function loadTemplates() {
    templatesError = "";
    const res = await listConventionTemplates();
    if (res.ok) {
      templates = res.data;
      selected = defaultTemplateKey(res.data);
    } else {
      templatesError = res.error;
    }
  }

  function close() {
    if (!busy) mode = null;
  }

  // Esc asks to close; a save in flight finishes first.
  function onCancel(e: Event) {
    e.preventDefault();
    close();
  }

  function onClose() {
    mode = null;
    const back = opener?.isConnected ? opener : (returnFocus?.() ?? null);
    opener = null;
    back?.focus();
  }

  // A click on the backdrop lands on the <dialog> itself.
  function onClick(e: MouseEvent) {
    if (e.target === dialogEl) close();
  }

  // The document view's shortcuts (E to edit, Esc back to the list) listen
  // on window; keys pressed in here stop at the dialog.
  function onKeydown(e: KeyboardEvent) {
    e.stopPropagation();
    if (e.key === "Tab") trapTab(e);
  }

  // A modal dialog still lets Tab run out to the browser's own controls;
  // wrap it around inside instead.
  function trapTab(e: KeyboardEvent) {
    if (!dialogEl) return;
    const focusable = [
      ...dialogEl.querySelectorAll<HTMLElement>(
        'button:not([disabled]), a[href], input:not([disabled]), select, textarea, [tabindex]:not([tabindex="-1"])',
      ),
    ].filter((el) => el.getClientRects().length > 0);
    if (focusable.length === 0) return;
    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    const active = document.activeElement;
    const outside = !dialogEl.contains(active);
    if (e.shiftKey && (active === first || outside)) {
      e.preventDefault();
      last.focus();
    } else if (!e.shiftKey && (active === last || outside)) {
      e.preventDefault();
      first.focus();
    }
  }

  async function confirm() {
    if (busy || mode === null) return;
    busy = true;
    const ok =
      mode === "template"
        ? chosen !== null && (await onReplace(chosen))
        : await onSetEnabled(mode === "on");
    busy = false;
    if (ok) mode = null;
  }

  const heading = $derived(
    mode === "template"
      ? "Change the conventions template"
      : mode === "off"
        ? "Leave conventions out of briefings?"
        : "Put conventions back in briefings?",
  );
</script>

<!-- svelte-ignore a11y_no_noninteractive_element_interactions a11y_click_events_have_key_events -->
<dialog
  bind:this={dialogEl}
  class="conventions-dialog m-auto p-0 border-0 sm:border sm:border-[var(--border)] sm:rounded-xl
         bg-[var(--surface)] text-[var(--text)] shadow-[0_16px_48px_rgba(0,0,0,0.28)]
         w-full h-full max-w-none max-h-none
         sm:h-auto sm:max-h-[86dvh] {mode === 'template' ? 'sm:max-w-[920px] sm:w-[calc(100%-2rem)]' : 'sm:max-w-[460px]'}"
  aria-labelledby="conventions-dialog-title"
  data-testid="conventions-dialog"
  data-mode={mode ?? undefined}
  oncancel={onCancel}
  onclose={onClose}
  onclick={onClick}
  onkeydown={onKeydown}
>
  {#if mode !== null}
    <div class="h-full sm:h-auto sm:max-h-[86dvh] flex flex-col">
      <div class="shrink-0 flex items-center gap-2 px-4 sm:px-6 pt-[max(0.875rem,env(safe-area-inset-top))] sm:pt-5 pb-1">
        <h2 id="conventions-dialog-title" class="flex-1 text-heading font-medium text-[var(--text)]">{heading}</h2>
        <button
          type="button"
          class="touch-target size-8 -mr-1 grid place-items-center rounded-md text-[var(--text-muted)]
                 hover:text-[var(--text)] hover:bg-[var(--bg-subtle)] transition-colors"
          aria-label="Close"
          onclick={close}
        >
          <X size={18} />
        </button>
      </div>

      <div class="flex-1 min-h-0 overflow-y-auto overscroll-contain px-4 sm:px-6 pb-5" style="--picker-bg: var(--surface)">
        {#if mode === "template"}
          <p class="text-body-sm text-[var(--text-muted)] leading-relaxed mb-5 max-w-[68ch]">
            The template's text replaces everything on {pageIdentifier}. The
            current text stays in the page's history, so you can copy back
            anything you want to keep.
          </p>
          {#if templatesError}
            <p class="text-body-sm text-[var(--error)]" role="alert">
              Couldn't load the templates: {templatesError}.
              <button class="text-[var(--accent)] hover:underline" onclick={() => void loadTemplates()}>Try again</button>
            </p>
          {:else}
            <div data-autofocus tabindex="-1" class="outline-none">
              <ConventionsTemplatePicker {templates} bind:selected disabled={busy} />
            </div>
          {/if}
        {:else if mode === "off"}
          <p class="text-body-sm text-[var(--text-muted)] leading-relaxed">
            Agents stop getting {pageIdentifier} at the start of their sessions.
            The page and its text stay here, and anyone in the project can still
            read and edit it. You can put it back in from this menu.
          </p>
        {:else}
          <p class="text-body-sm text-[var(--text-muted)] leading-relaxed">
            Agents get {pageIdentifier} at the top of their briefing when a
            session starts, as written now.
          </p>
        {/if}
      </div>

      <div
        class="shrink-0 flex flex-wrap items-center justify-end gap-2 px-4 sm:px-6 py-3 border-t border-[var(--border)]
               pb-[max(0.75rem,env(safe-area-inset-bottom))] sm:pb-3"
      >
        <button
          type="button"
          class="text-body-sm text-[var(--text-muted)] px-3 py-1.5 rounded-md hover:bg-[var(--bg-subtle)] transition-colors"
          disabled={busy}
          onclick={close}
        >
          Cancel
        </button>
        <button
          type="button"
          data-autofocus={mode !== "template" ? "" : undefined}
          data-testid="conventions-dialog-confirm"
          class="text-body-sm font-medium px-3 py-1.5 rounded-md transition-opacity hover:opacity-90
                 disabled:opacity-50 disabled:cursor-not-allowed
                 {mode === 'template'
            ? 'text-[var(--error-text,white)] bg-[var(--error)]'
            : 'text-[var(--accent-text)] bg-[var(--accent)]'}"
          disabled={busy || (mode === "template" && chosen === null)}
          onclick={confirm}
        >
          {#if mode === "template"}
            {busy ? "Replacing…" : chosen ? `Replace with ${templateName(chosen)}` : "Replace text"}
          {:else if mode === "off"}
            {busy ? "Saving…" : "Leave out of briefings"}
          {:else}
            {busy ? "Saving…" : "Put back in briefings"}
          {/if}
        </button>
      </div>
    </div>
  {/if}
</dialog>

<style>
  .conventions-dialog::backdrop {
    background: rgb(0 0 0 / 0.3);
  }
</style>

<script module lang="ts">
  export type ConventionsDialogMode = "template" | "off" | "on";
</script>

<script lang="ts">
  // LIF-524: the deliberate changes to a conventions page, each confirmed
  // with what it does: replace the text with a template, or take the page out
  // of agents' briefings and put it back.

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
  }: {
    /** Which dialog is open; null when none is. */
    mode?: ConventionsDialogMode | null;
    pageIdentifier: string;
    /** Resolves true once the page holds the template's text. */
    onReplace: (template: ConventionTemplate) => Promise<boolean>;
    onSetEnabled: (enabled: boolean) => Promise<boolean>;
  } = $props();

  let templates = $state<ConventionTemplate[] | null>(null);
  let templatesError = $state("");
  let selected = $state<ConventionTemplateKey>("empty");
  let busy = $state(false);
  let dialogEl = $state<HTMLDivElement | null>(null);

  const chosen = $derived(templates?.find((t) => t.key === selected) ?? null);

  $effect(() => {
    if (mode === null) return;
    busy = false;
    if (mode === "template" && templates === null) void loadTemplates();
    void tick().then(() => (dialogEl?.querySelector<HTMLElement>("[data-autofocus]") ?? dialogEl)?.focus());
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

  // The dialog owns the keyboard while it is open: the document view's
  // shortcuts (Esc back to the list, E to edit) must not fire behind it.
  function onKeydown(e: KeyboardEvent) {
    e.stopPropagation();
    if (e.key === "Escape") {
      e.preventDefault();
      close();
    }
  }

  const heading = $derived(
    mode === "template"
      ? "Change the conventions template"
      : mode === "off"
        ? "Leave conventions out of briefings?"
        : "Put conventions back in briefings?",
  );
</script>

{#if mode !== null}
  <!-- svelte-ignore a11y_no_static_element_interactions a11y_click_events_have_key_events -->
  <div
    class="fixed inset-0 z-[100] bg-black/30 flex items-stretch sm:items-start justify-center sm:pt-[10dvh] sm:px-4"
    onclick={close}
    onkeydown={onKeydown}
  >
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <div
      bind:this={dialogEl}
      class="w-full flex flex-col bg-[var(--surface)] border-[var(--border)]
             shadow-[0_16px_48px_rgba(0,0,0,0.28)] overflow-hidden
             sm:border sm:rounded-xl sm:max-h-[80dvh]
             {mode === 'template' ? 'sm:max-w-[840px]' : 'sm:max-w-[460px] sm:h-auto'}"
      role="dialog"
      aria-modal="true"
      aria-labelledby="conventions-dialog-title"
      tabindex="-1"
      data-testid="conventions-dialog"
      data-mode={mode}
      onclick={(e) => e.stopPropagation()}
    >
      <div class="shrink-0 flex items-center gap-2 px-4 sm:px-5 pt-[max(0.875rem,env(safe-area-inset-top))] sm:pt-4 pb-1">
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

      <div class="flex-1 min-h-0 overflow-y-auto overscroll-contain px-4 sm:px-5 pb-4 flex flex-col gap-3">
        {#if mode === "template"}
          <p class="text-body-sm text-[var(--text-muted)] leading-relaxed">
            The template's text replaces everything on {pageIdentifier} now. The
            current text stays in the page's history, so you can copy back
            anything you want to keep.
          </p>
          {#if templatesError}
            <p class="text-body-sm text-[var(--error)]" role="alert">
              Couldn't load the templates: {templatesError}.
              <button class="text-[var(--accent)] hover:underline" onclick={() => void loadTemplates()}>Try again</button>
            </p>
          {:else}
            <ConventionsTemplatePicker {templates} bind:selected disabled={busy} previewHeight="min(22rem, 40dvh)" />
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
        class="shrink-0 flex flex-wrap items-center justify-end gap-2 px-4 sm:px-5 py-3 border-t border-[var(--border)]
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
            ? 'text-[var(--accent-text,white)] bg-[var(--error)]'
            : 'text-[var(--accent-text,white)] bg-[var(--accent)]'}"
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
  </div>
{/if}

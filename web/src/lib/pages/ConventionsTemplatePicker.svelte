<script lang="ts">
  // LIF-524: choose a conventions template while reading its full text.
  // Shared by the create-project form, the Conventions tab's first start and
  // its "Change template" dialog. It only picks; the caller owns the action
  // that applies the choice.

  import type { ConventionTemplate, ConventionTemplateKey } from "../api";
  import Markdown from "../Markdown.svelte";
  import Skeleton from "../Skeleton.svelte";
  import { templateName } from "./conventions";

  let {
    templates,
    selected = $bindable<ConventionTemplateKey>("empty"),
    label = "Conventions template",
    previewHeight = "22rem",
    disabled = false,
  }: {
    /** null while the templates load. */
    templates: ConventionTemplate[] | null;
    selected?: ConventionTemplateKey;
    /** The radio group's accessible name. */
    label?: string;
    /** Max height of the preview before it scrolls. */
    previewHeight?: string;
    disabled?: boolean;
  } = $props();

  const current = $derived(templates?.find((t) => t.key === selected) ?? null);
  let groupEl = $state<HTMLDivElement | null>(null);

  // Arrow keys move the choice, as in a native radio group.
  function onKeydown(e: KeyboardEvent) {
    if (!templates || templates.length === 0 || disabled) return;
    const step = e.key === "ArrowDown" || e.key === "ArrowRight" ? 1 : e.key === "ArrowUp" || e.key === "ArrowLeft" ? -1 : 0;
    if (step === 0) return;
    e.preventDefault();
    const index = Math.max(0, templates.findIndex((t) => t.key === selected));
    const next = templates[(index + step + templates.length) % templates.length];
    selected = next.key;
    groupEl?.querySelector<HTMLElement>(`[data-template="${next.key}"]`)?.focus();
  }
</script>

<div class="@container" data-testid="template-picker">
  <div class="grid grid-cols-1 gap-3 @2xl:grid-cols-[minmax(0,15rem)_minmax(0,1fr)]">
    <div
      bind:this={groupEl}
      class="flex flex-col gap-2"
      role="radiogroup"
      aria-label={label}
      tabindex="-1"
      onkeydown={onKeydown}
    >
      {#if templates === null}
        {#each [0, 1, 2, 3] as i (i)}
          <div class="rounded-md border border-[var(--border)] px-3 py-2.5 flex flex-col gap-1.5">
            <Skeleton variant="bar" class="h-3.5 w-28" />
            <Skeleton variant="bar" class="h-3 w-full" />
          </div>
        {/each}
      {:else}
        {#each templates as t (t.key)}
          {@const isSelected = selected === t.key}
          <button
            type="button"
            role="radio"
            aria-checked={isSelected}
            tabindex={isSelected ? 0 : -1}
            data-template={t.key}
            {disabled}
            class="flex items-start gap-3 rounded-md border px-3 py-2.5 text-left transition-colors
                   focus-visible:outline-none focus-visible:shadow-[0_0_0_3px_var(--accent-subtle)]
                   disabled:opacity-60 disabled:cursor-not-allowed
                   {isSelected
              ? 'border-[var(--accent)] bg-[var(--accent-subtle)]'
              : 'border-[var(--border)] bg-[var(--bg-subtle)] hover:border-[var(--text-faint)]'}"
            onclick={() => (selected = t.key)}
          >
            <span
              class="mt-0.5 size-4 shrink-0 rounded-full border-2 grid place-items-center
                     {isSelected ? 'border-[var(--accent)]' : 'border-[var(--text-faint)]'}"
            >
              {#if isSelected}<span class="size-2 rounded-full bg-[var(--accent)]"></span>{/if}
            </span>
            <span class="flex-1 min-w-0">
              <span class="block text-body-sm font-medium {isSelected ? 'text-[var(--accent)]' : 'text-[var(--text)]'}">
                {templateName(t)}
              </span>
              {#if t.summary}
                <span class="block text-caption text-[var(--text-muted)] leading-snug mt-0.5">{t.summary}</span>
              {/if}
            </span>
          </button>
        {/each}
      {/if}
    </div>

    <section
      class="min-w-0 rounded-md border border-[var(--border)] bg-[var(--bg)] flex flex-col"
      aria-label={current ? `${templateName(current)}, full text` : "Template text"}
      data-testid="template-preview"
      data-template={current?.key}
    >
      <p class="shrink-0 px-4 pt-3 pb-2 text-micro font-semibold uppercase tracking-widest text-[var(--text-faint)] border-b border-[var(--border)]">
        {current ? `${templateName(current)}: full text` : "Full text"}
      </p>
      <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
      <div
        class="px-4 py-3 overflow-y-auto overscroll-contain"
        style:max-height={previewHeight}
        tabindex="0"
      >
        {#if templates === null}
          <div class="flex flex-col gap-2">
            <Skeleton variant="bar" class="h-3.5 w-1/2" />
            <Skeleton variant="bar" class="h-3 w-full" />
            <Skeleton variant="bar" class="h-3 w-5/6" />
          </div>
        {:else if current && current.content.trim() !== ""}
          <Markdown content={current.content} class="text-body-sm" />
        {:else if current?.key === "empty"}
          <p class="text-body-sm text-[var(--text-muted)]">
            An empty page. You write the rules yourself, in your own words.
          </p>
        {:else}
          <p class="text-body-sm text-[var(--text-muted)]">The text couldn't load. You can still pick this template.</p>
        {/if}
      </div>
    </section>
  </div>
</div>

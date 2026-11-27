<script lang="ts">
  // LIF-524: choose a conventions template by reading it. The selected
  // template's text is laid out like the page it becomes, at full length,
  // beside a short list of the four (a row of choices on a narrow screen).
  // Shared by the Conventions tab, its "Change template" dialog and the
  // create-project form. It only picks; the caller owns what applies the
  // choice, and may put that action in the text's header.

  import type { Snippet } from "svelte";
  import type { ConventionTemplate, ConventionTemplateKey } from "../api";
  import Markdown from "../Markdown.svelte";
  import Skeleton from "../Skeleton.svelte";
  import { wordCount } from "../detail/outline";
  import { templateName, templateTagline } from "./conventions";

  let {
    templates,
    selected = $bindable<ConventionTemplateKey>("empty"),
    label = "Conventions template",
    disabled = false,
    excerpt = false,
    action,
  }: {
    /** null while the templates load. */
    templates: ConventionTemplate[] | null;
    selected?: ConventionTemplateKey;
    /** The radio group's accessible name. */
    label?: string;
    disabled?: boolean;
    /** Show the opening of the text with a "Show the full text" control,
     *  for a form where the text is one field among several. */
    excerpt?: boolean;
    /** Rendered at the end of the text's header, which stays in view while
     *  the text scrolls. */
    action?: Snippet<[ConventionTemplate]>;
  } = $props();

  const current = $derived(templates?.find((t) => t.key === selected) ?? null);
  const words = $derived(current ? wordCount(current.content) : 0);
  let expanded = $state(false);
  let groupEl = $state<HTMLDivElement | null>(null);

  function choose(key: ConventionTemplateKey) {
    if (disabled) return;
    selected = key;
  }

  // Arrow keys move the choice, as in a native radio group.
  function onKeydown(e: KeyboardEvent) {
    if (!templates || templates.length === 0 || disabled) return;
    const step =
      e.key === "ArrowDown" || e.key === "ArrowRight" ? 1 : e.key === "ArrowUp" || e.key === "ArrowLeft" ? -1 : 0;
    if (step === 0) return;
    e.preventDefault();
    const index = Math.max(0, templates.findIndex((t) => t.key === selected));
    const next = templates[(index + step + templates.length) % templates.length];
    selected = next.key;
    const el = groupEl?.querySelector<HTMLElement>(`[role="radio"][data-template="${next.key}"]`);
    el?.focus();
    el?.scrollIntoView({ block: "nearest", inline: "nearest" });
  }
</script>

<div class="picker @container" data-testid="template-picker">
  <div class="grid grid-cols-1 gap-4 @[44rem]:grid-cols-[12rem_minmax(0,1fr)] @[44rem]:gap-10">
    <!-- The choices: a list beside the text when there is room, a row of
         choices above it when there is not. -->
    <div
      bind:this={groupEl}
      class="choices min-w-0 flex gap-1.5 overflow-x-auto overscroll-x-contain -mx-1 px-1 pb-1
             @[44rem]:flex-col @[44rem]:gap-0.5 @[44rem]:overflow-visible @[44rem]:mx-0 @[44rem]:px-0 @[44rem]:pb-0
             @[44rem]:sticky @[44rem]:top-3 @[44rem]:self-start"
      role="radiogroup"
      aria-label={label}
      tabindex="-1"
      onkeydown={onKeydown}
    >
      {#if templates === null}
        {#each [0, 1, 2, 3] as i (i)}
          <div class="shrink-0 w-32 @[44rem]:w-auto px-3 py-2 flex flex-col gap-1.5">
            <Skeleton variant="bar" class="h-3.5 w-24" />
            <Skeleton variant="bar" class="hidden @[44rem]:block h-3 w-32" />
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
            class="choice shrink-0 text-left rounded-md transition-colors
                   px-3 py-1.5 border @[44rem]:border-0 @[44rem]:py-2
                   focus-visible:outline-none focus-visible:shadow-[0_0_0_3px_var(--accent-subtle)]
                   disabled:cursor-not-allowed
                   {isSelected
              ? 'bg-[var(--accent-subtle)] border-[var(--accent)]'
              : 'border-[var(--border)] hover:bg-[var(--bg-subtle)]'}"
            onclick={() => choose(t.key)}
          >
            <span
              class="block whitespace-nowrap text-body-sm font-medium
                     {isSelected ? 'text-[var(--accent)]' : 'text-[var(--text)]'}"
            >
              {templateName(t)}
            </span>
            <span class="hidden @[44rem]:block text-caption text-[var(--text-muted)] leading-snug mt-0.5">
              {templateTagline(t)}
            </span>
          </button>
        {/each}
      {/if}
    </div>

    <!-- The text, as the page it becomes. -->
    <section
      class="min-w-0"
      aria-label={current ? `${templateName(current)}, the full text` : "Template text"}
      data-testid="template-preview"
      data-template={current?.key}
    >
      <header
        class="doc-head z-[1] flex items-center gap-3 border-b border-[var(--border)] pb-3 mb-5
               {action ? 'sticky top-0 pt-3 -mt-3' : ''}"
      >
        <div class="min-w-0 flex-1">
          {#if current}
            <h2 class="text-heading font-medium text-[var(--text)] leading-tight truncate">{templateName(current)}</h2>
            <p class="text-caption text-[var(--text-muted)] mt-0.5">
              {current.key === "empty"
                ? "An empty page"
                : words > 0
                  ? `${words.toLocaleString()} words · ${templateTagline(current)}`
                  : templateTagline(current)}
            </p>
          {:else}
            <Skeleton variant="bar" class="h-5 w-40" />
          {/if}
        </div>
        {#if action && current}
          <div class="shrink-0">{@render action(current)}</div>
        {/if}
      </header>

      {#if templates === null}
        <div class="flex flex-col gap-2.5 max-w-[68ch]">
          <Skeleton variant="bar" class="h-3.5 w-full" />
          <Skeleton variant="bar" class="h-3.5 w-5/6" />
          <Skeleton variant="bar" class="h-3.5 w-full mt-3" />
          <Skeleton variant="bar" class="h-3.5 w-2/3" />
        </div>
      {:else if current?.key === "empty"}
        <!-- A blank page, drawn as one: faint ruled lines, and what it means. -->
        <div class="blank max-w-[68ch]" data-testid="template-blank">
          <div class="flex flex-col gap-3 mb-5" aria-hidden="true">
            <span class="block h-2.5 w-2/5 rounded-full bg-[var(--bg-subtle)]"></span>
            <span class="block h-2 w-full rounded-full bg-[var(--bg-subtle)]"></span>
            <span class="block h-2 w-11/12 rounded-full bg-[var(--bg-subtle)]"></span>
            <span class="block h-2 w-3/5 rounded-full bg-[var(--bg-subtle)]"></span>
          </div>
          <p class="text-body text-[var(--text)]">Nothing to start from. You write the rules in your own words.</p>
          <p class="text-body-sm text-[var(--text-muted)] mt-1">
            Agents read whatever the page says, so even a few lines help.
          </p>
        </div>
      {:else if current && current.content.trim() !== ""}
        <div
          class="max-w-[68ch] {excerpt && !expanded ? 'excerpt max-h-[16rem] overflow-hidden' : ''}"
          data-testid="template-text"
        >
          <Markdown content={current.content} />
        </div>
        {#if excerpt}
          <button
            type="button"
            class="mt-2 text-body-sm font-medium text-[var(--accent)] hover:underline"
            aria-expanded={expanded}
            onclick={() => (expanded = !expanded)}
          >
            {expanded ? "Show less" : "Show the full text"}
          </button>
        {/if}
      {:else}
        <p class="text-body-sm text-[var(--text-muted)]">The text couldn't load. You can still pick this template.</p>
      {/if}
    </section>
  </div>
</div>

<style>
  /* The sticky header covers the text scrolling under it. */
  .doc-head {
    background: var(--picker-bg, var(--bg));
  }
  /* The cut-off text fades out, so it reads as more to come. */
  .excerpt {
    -webkit-mask-image: linear-gradient(to bottom, #000 60%, transparent);
    mask-image: linear-gradient(to bottom, #000 60%, transparent);
  }
  .choices {
    scrollbar-width: none;
  }
</style>

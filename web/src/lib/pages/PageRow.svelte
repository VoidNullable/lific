<script lang="ts">
  // One page in the Pages list: a single line (title, status word, labels,
  // then fixed columns for links, editor, id and date), with the excerpt
  // under it when excerpts are on. The parent's list is a size container,
  // so the columns drop out one by one as the list narrows.
  import { Check, FileText, Link2, Pin } from "lucide-svelte";
  import type { Label, PageMeta } from "../api";
  import TimeAgo from "../TimeAgo.svelte";
  import Tooltip from "../Tooltip.svelte";
  import { longpress } from "../actions/longpress";
  import { safeLabelColor } from "../labelColors";
  import { copyToClipboard } from "../clipboard";
  import EditorMark from "./EditorMark.svelte";
  import { describeLinks, excerptOf, linkTotal, personLabel, STATUS_LABEL, type ListPage } from "./list";
  import type { Snippet } from "svelte";

  let {
    page,
    meta,
    labels,
    href,
    folderLabel = null,
    excerpts = true,
    focused = false,
    selected = false,
    selecting = false,
    changed = false,
    flashing = false,
    dragging = false,
    canEdit = false,
    showStatus = true,
    snippet = null,
    note,
    onActivate,
    onOpen,
    onToggleSelect,
    onTogglePin,
    onContextMenu,
    onLongPress,
    onHover,
    onDragStart,
    onDragEnd,
  }: {
    page: ListPage;
    meta: PageMeta | undefined;
    labels: Label[];
    href: string;
    /** Shown when the list is not grouped by folder. */
    folderLabel?: string | null;
    excerpts?: boolean;
    focused?: boolean;
    selected?: boolean;
    /** Something is selected, so every row shows its checkbox. */
    selecting?: boolean;
    changed?: boolean;
    flashing?: boolean;
    dragging?: boolean;
    canEdit?: boolean;
    /** Off in a tab that already names the status. */
    showStatus?: boolean;
    /** Search: the matching text, shown in place of the excerpt. */
    snippet?: string | null;
    /** Extra trailing content for a tab (Drafts: the age and Mark active). */
    note?: Snippet;
    /** Plain click on the row. */
    onActivate: (page: ListPage) => void;
    onOpen: (page: ListPage) => void;
    onToggleSelect: (page: ListPage, shift: boolean) => void;
    onTogglePin: (page: ListPage) => void;
    onContextMenu: (page: ListPage, e: MouseEvent) => void;
    /** Press and hold on touch. */
    onLongPress: (page: ListPage, e: PointerEvent) => void;
    onHover?: (page: ListPage, e: MouseEvent) => void;
    onDragStart?: (page: ListPage, e: DragEvent) => void;
    onDragEnd?: () => void;
  } = $props();

  const excerpt = $derived(snippet ?? (excerpts ? excerptOf(page, meta) : ""));
  const links = $derived(linkTotal(meta));
  const editor = $derived(meta?.updated_by ?? null);
  const labelColor = (name: string) => {
    const l = labels.find((x) => x.name === name);
    return l ? safeLabelColor(l.color) : "var(--text-faint)";
  };

  function onClick(e: MouseEvent) {
    if (e.defaultPrevented) return;
    const t = e.target as HTMLElement;
    // Modified clicks on the title link keep the browser's behavior.
    if (t.closest("a") && (e.metaKey || e.ctrlKey || e.altKey || e.button !== 0)) return;
    if (t.closest("button,[role=button]") && !t.closest("a")) return;
    e.preventDefault();
    if (e.shiftKey && canEdit) {
      onToggleSelect(page, true);
      return;
    }
    onActivate(page);
  }
</script>

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
<div
  class="page-row group relative flex flex-col justify-center gap-0.5 px-3 sm:px-5
         {excerpt ? 'sm:min-h-[3.4rem] min-h-[2.75rem] py-2' : 'min-h-[2.75rem] sm:min-h-[2.6rem] py-1.5'}
         border-l-2 border-b border-b-[color-mix(in_srgb,var(--border)_60%,transparent)]
         transition-colors duration-100 cursor-default select-none
         {focused ? 'border-l-[var(--accent)]' : 'border-l-transparent'}
         {selected
    ? 'bg-[var(--accent-subtle)]'
    : focused
      ? 'bg-[color-mix(in_srgb,var(--accent-subtle)_70%,transparent)]'
      : 'hover:bg-[var(--bg-subtle)]'}
         {flashing ? 'row-flash' : ''}
         {dragging ? 'opacity-40' : ''}"
  role="group"
  aria-label={page.title || page.identifier}
  data-page-id={page.id}
  data-testid="page-row"
  draggable={canEdit}
  ondragstart={(e) => onDragStart?.(page, e)}
  ondragend={() => onDragEnd?.()}
  onclick={onClick}
  ondblclick={(e) => {
    if ((e.target as HTMLElement).closest("button")) return;
    onOpen(page);
  }}
  oncontextmenu={(e) => onContextMenu(page, e)}
  onmouseenter={(e) => onHover?.(page, e)}
  use:longpress={{ onLongPress: (e) => onLongPress(page, e) }}
>
  <div class="flex items-center gap-2.5 min-w-0">
    <!-- Lead: the page icon, or the checkbox on hover and while selecting. -->
    <span class="relative size-4 shrink-0 grid place-items-center">
      <FileText
        size={15}
        class="text-[var(--text-faint)] transition-opacity
               {canEdit && (selected || selecting) ? 'opacity-0' : ''}
               {canEdit ? '[@media(hover:hover)]:group-hover:opacity-0' : ''}"
      />
      {#if canEdit}
        <button
          class="absolute inset-0 rounded-[5px] border-[1.5px] grid place-items-center
                 transition-[opacity,background,border-color] duration-150
                 {selected
            ? 'opacity-100 bg-[var(--accent)] border-[var(--accent)] text-[var(--accent-text)]'
            : `${selecting ? 'opacity-100' : 'opacity-0 [@media(hover:hover)]:group-hover:opacity-100'}
               border-[var(--text-faint)] text-transparent hover:border-[var(--text)]`}"
          role="checkbox"
          aria-checked={selected}
          aria-label={`Select ${page.identifier}`}
          title={selected ? "Deselect" : "Select  ·  X"}
          onclick={(e) => {
            e.stopPropagation();
            onToggleSelect(page, e.shiftKey);
          }}
        >
          <Check size={11} strokeWidth={3} />
        </button>
      {/if}
    </span>

    <span class="w-1.5 -ml-1 shrink-0 flex items-center justify-center">
      {#if changed}
        <Tooltip content="Updated since you last looked">
          <span class="block size-1.5 rounded-full bg-[var(--accent)]" data-testid="page-changed"></span>
        </Tooltip>
        <span class="sr-only">Updated since you last looked</span>
      {/if}
    </span>

    <span class="flex-1 min-w-0 flex items-center gap-2">
      <a
        {href}
        draggable="false"
        class="min-w-0 truncate text-body text-[var(--text)] font-medium hover:underline decoration-[var(--text-faint)] underline-offset-2"
      >
        {page.title || "Untitled"}
      </a>
      {#if showStatus && page.status !== "active"}
        <span
          class="shrink-0 text-micro font-medium px-1.5 py-px rounded
                 {page.status === 'complete'
            ? 'text-[var(--success)] bg-[color-mix(in_srgb,var(--success)_12%,transparent)]'
            : 'text-[var(--text-muted)] bg-[var(--bg-subtle)] ring-1 ring-inset ring-[var(--border)]'}"
          data-status={page.status}
        >
          {STATUS_LABEL[page.status] ?? page.status}
        </span>
      {/if}
      {#if page.pinned}
        <Pin size={12} class="shrink-0 fill-current text-[var(--accent)]" aria-label="Pinned" />
      {/if}
      {#if page.labels.length > 0}
        <span class="hidden @md:flex items-center gap-2 shrink min-w-0 overflow-hidden">
          {#each page.labels.slice(0, 2) as name (name)}
            <span class="flex items-center gap-1 min-w-0 text-caption text-[var(--text-muted)]">
              <span class="size-2 rounded-full shrink-0" style="background: {labelColor(name)}"></span>
              <span class="truncate">{name}</span>
            </span>
          {/each}
          {#if page.labels.length > 2}
            <span class="text-caption text-[var(--text-faint)] shrink-0">+{page.labels.length - 2}</span>
          {/if}
        </span>
        <span class="flex @md:hidden items-center -space-x-0.5 shrink-0" aria-label={page.labels.join(", ")}>
          {#each page.labels.slice(0, 3) as name (name)}
            <span class="size-2 rounded-full ring-1 ring-[var(--surface)]" style="background: {labelColor(name)}"></span>
          {/each}
        </span>
      {/if}
    </span>

    {#if folderLabel}
      <span class="hidden @2xl:block shrink-0 max-w-[10rem] truncate text-caption text-[var(--text-faint)]">
        {folderLabel}
      </span>
    {/if}

    {#if note}
      {@render note()}
    {/if}

    <span class="hidden @lg:flex w-9 shrink-0 justify-end">
      {#if links > 0}
        <Tooltip content={describeLinks(meta)}>
          <span class="flex items-center gap-1 text-caption tabular-nums text-[var(--text-muted)]" data-testid="page-links">
            <Link2 size={12} class="text-[var(--text-faint)]" />{links}
          </span>
        </Tooltip>
      {/if}
    </span>

    <span class="hidden @sm:flex w-[18px] shrink-0 justify-center">
      {#if editor}
        <EditorMark person={editor} tip={`Last edited by ${personLabel(editor)}`} />
      {/if}
    </span>

    <button
      class="hidden @xl:block w-[5.5rem] shrink-0 text-left text-caption font-mono truncate
             text-[var(--text-faint)] hover:text-[var(--accent)] transition-colors"
      title="Copy {page.identifier}"
      onclick={(e) => {
        e.stopPropagation();
        void copyToClipboard(page.identifier);
      }}
    >
      {page.identifier}
    </button>

    <TimeAgo
      class="w-[3.75rem] shrink-0 text-right text-caption tabular-nums text-[var(--text-faint)]
             group-hover:text-[var(--text-muted)] transition-colors"
      date={page.updated_at}
    />

    {#if canEdit}
      <button
        class="hidden [@media(hover:hover)]:grid size-6 -mr-1.5 shrink-0 place-items-center rounded-md
               text-[var(--text-faint)] hover:text-[var(--accent)] hover:bg-[var(--bg-subtle)]
               opacity-0 group-hover:opacity-100 focus-visible:opacity-100 transition-opacity"
        title={page.pinned ? "Unpin  ·  P" : "Pin  ·  P"}
        aria-label={page.pinned ? `Unpin ${page.identifier}` : `Pin ${page.identifier}`}
        onclick={(e) => {
          e.stopPropagation();
          onTogglePin(page);
        }}
      >
        <Pin size={13} class={page.pinned ? "fill-current" : ""} />
      </button>
    {/if}
  </div>

  {#if excerpt}
    <p
      class="pl-[1.75rem] pr-2 text-body-sm text-[var(--text-muted)] truncate {snippet ? 'italic' : 'max-sm:hidden'}"
      data-testid="page-excerpt"
    >
      {excerpt}
    </p>
  {/if}
</div>

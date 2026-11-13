<script lang="ts">
  // The Markdown editor as a Svelte component: a live-preview CodeMirror
  // surface (see ./cm) plus everything around it that is DOM rather than
  // text: the selection toolbar, the "Open" popover for links, issue hover
  // cards, the image viewer, and the attachment composer (paste, drop at the
  // pointer, the picker, the pending strip, the big-paste offer).
  //
  // It knows nothing about saving. The host gets every edit through
  // `onChange` and drives the text through the handle `onReady` hands over.
  //
  // Until the editor chunk arrives the text is shown through the read-only
  // renderer, so the page never flashes empty.

  import { mount, onDestroy, unmount, type Snippet } from "svelte";
  import type { ChangeSet } from "@codemirror/state";
  import type { EditorView } from "@codemirror/view";
  import { ExternalLink } from "lucide-svelte";
  import Markdown from "../Markdown.svelte";
  import IssueHoverCard from "../IssueHoverCard.svelte";
  import ImageLightbox, { type LightboxImage } from "../attachments/ImageLightbox.svelte";
  import AttachComposer from "../attachments/AttachComposer.svelte";
  import AttachTrigger from "../attachments/AttachTrigger.svelte";
  import { createComposerAttachments } from "../attachments/composer.svelte";
  import type { AttachmentEntity } from "../api";
  import { attachmentThumbnailUrl, attachmentUrl } from "../api";
  import { THUMBNAIL_EDGE } from "../imageSize";
  import { projectCodeOf, refKind, routeFor } from "../references";
  import FormatToolbar from "./FormatToolbar.svelte";
  import { loadEditor } from "./loadEditor";
  import type { FormatAction } from "./cm/commands";
  import type { LinkAt } from "./cm/links";
  import type { MarkdownEditorHandle } from "./cm/index";

  type EditorModule = Awaited<ReturnType<typeof loadEditor>>;

  let {
    doc,
    placeholder = "",
    label = "Body",
    attachTo = null,
    fallbackClass = "",
    onChange,
    onReady,
    onSaveNow,
    onEscape,
    onFocusChange,
    onQuote,
    status,
    footerVisible = false,
  }: {
    /** Initial text. Later changes go through the handle, not this prop. */
    doc: string;
    placeholder?: string;
    label?: string;
    attachTo?: { entity_type: AttachmentEntity; entity_id: number } | null;
    fallbackClass?: string;
    /** `discrete`: a one-off action (ticking a task) rather than typing. */
    onChange?: (changes: ChangeSet, discrete: boolean) => void;
    onReady?: (handle: MarkdownEditorHandle, mod: EditorModule) => void;
    onSaveNow?: () => void;
    onEscape?: () => void;
    onFocusChange?: (focused: boolean) => void;
    /** Push the selected text into the comment box. */
    onQuote?: (text: string) => void;
    /** Rendered at the end of the footer row (save state, conflicts). */
    status?: Snippet<[{ focused: boolean }]>;
    /** Keep the footer showing without focus (an unsaved or failed save). */
    footerVisible?: boolean;
  } = $props();

  // The text the fallback shows until the editor arrives.
  // svelte-ignore state_referenced_locally
  const initial = doc;

  let host = $state<HTMLDivElement | null>(null);
  let handle = $state<MarkdownEditorHandle | null>(null);
  let mod: EditorModule | null = null;
  let loadError = $state(false);
  let focused = $state(false);
  let destroyed = false;
  /** A focus request made before the editor finished loading. */
  let pendingFocus: "end" | "keep" | null = null;
  /** Snippets inserted before the editor finished loading. */
  const pendingInserts: string[] = [];

  // ── attachments ──────────────────────────────────────────

  const attach = createComposerAttachments({
    el: () => handle?.caret() ?? null,
    text: {
      read: () => handle?.text() ?? initial,
      write: (next, caret) => handle?.setText(next, caret ?? null),
    },
    link: () => attachTo,
  });

  // ── preview services (images, rendered blocks) ───────────

  const ATTACHMENT_RE = /^(?:https?:\/\/[^/]+)?\/api\/attachments\/(\d+)\/?$/;

  function imageSrc(url: string, width: number | null) {
    const m = url.match(ATTACHMENT_RE);
    if (!m) return { src: url, fallback: null };
    const id = Number(m[1]);
    if (width && width > THUMBNAIL_EDGE) return { src: attachmentUrl(id), fallback: null };
    return { src: attachmentThumbnailUrl(id), fallback: attachmentUrl(id) };
  }

  let lightbox = $state<{ images: LightboxImage[]; start: number } | null>(null);
  const IMAGE_RE = /!\[([^\]\n]*)\]\(([^)\s]+)(?:\s+"[^"]*")?\)/g;

  function openImage(view: EditorView, pos: number) {
    const text = view.state.doc.toString();
    const images: LightboxImage[] = [];
    let start = 0;
    for (const m of text.matchAll(IMAGE_RE)) {
      const from = m.index ?? 0;
      const to = from + m[0].length;
      const id = m[2].match(ATTACHMENT_RE)?.[1];
      if (pos >= from && pos <= to) start = images.length;
      images.push({
        src: id ? attachmentUrl(Number(id)) : m[2],
        alt: m[1].replace(/\s*\|\s*\d+\s*$/, ""),
      });
    }
    if (images.length > 0) lightbox = { images, start };
  }

  function renderBlock(dom: HTMLElement, source: string) {
    let component: ReturnType<typeof mount> | null = null;
    let gone = false;
    // Mounted from a microtask: the editor can redraw inside a Svelte effect
    // (a remote edit applied from a prop change), and a component mounted
    // there would be tied to that effect's lifetime.
    queueMicrotask(() => {
      if (gone) return;
      component = mount(Markdown, { target: dom, props: { content: source } });
    });
    return () => {
      gone = true;
      if (component) void unmount(component);
    };
  }

  // ── links ────────────────────────────────────────────────

  function hrefFor(target: string, kind: "url" | "ref"): { url: string; internal: boolean } {
    if (kind === "ref") {
      const marker = /-(DOC|PLAN)-\d+$/.exec(target)?.[1];
      return { url: routeFor(projectCodeOf(target), refKind(marker ? `${marker}-` : undefined), target), internal: true };
    }
    const m = target.match(ATTACHMENT_RE);
    if (m) return { url: attachmentUrl(Number(m[1])), internal: false };
    if (target.startsWith("#")) return { url: target, internal: true };
    return { url: target, internal: false };
  }

  function openLink(target: string, kind: "url" | "ref", newTab = false) {
    const { url, internal } = hrefFor(target, kind);
    if (internal && !newTab) {
      location.hash = url.replace(/^#/, "");
      return;
    }
    const absolute = internal ? `${location.origin}${location.pathname}${url}` : url;
    window.open(absolute, "_blank", "noopener,noreferrer");
  }

  // ── floating UI: toolbar and link popover ───────────────

  const coarse = typeof matchMedia !== "undefined" && matchMedia("(pointer: coarse)").matches;
  let toolbar = $state<{ x: number; y: number; active: Set<FormatAction> } | null>(null);
  let linkPop = $state<{ x: number; y: number; link: LinkAt } | null>(null);

  function updateFloating(view: EditorView) {
    if (!mod || !view.hasFocus) {
      toolbar = null;
      linkPop = null;
      return;
    }
    const sel = view.state.selection.main;
    if (!sel.empty) {
      linkPop = null;
      const a = view.coordsAtPos(sel.from);
      const b = view.coordsAtPos(sel.to, -1);
      if (!a || !b) {
        toolbar = null;
        return;
      }
      const sameLine = Math.abs(a.top - b.top) < 4;
      const x = sameLine ? (a.left + b.right) / 2 : a.left + 140;
      const y = coarse ? b.bottom : Math.min(a.top, b.top);
      toolbar = { x, y, active: mod.activeFormats(view) };
      return;
    }
    toolbar = null;
    const link = mod.linkAt(view.state, sel.head);
    if (!link) {
      linkPop = null;
      return;
    }
    const c = view.coordsAtPos(link.from);
    linkPop = c ? { x: c.left, y: c.bottom, link } : null;
  }

  function runFormat(action: FormatAction) {
    if (!handle || !mod) return;
    mod.formatCommands[action](handle.view);
  }

  function quoteSelection() {
    if (!handle || !onQuote) return;
    const sel = handle.view.state.selection.main;
    const text = handle.view.state.sliceDoc(sel.from, sel.to);
    if (!text.trim()) return;
    onQuote(text);
    handle.view.dispatch({ selection: { anchor: sel.to } });
  }

  function onWindowScroll() {
    if (handle && (toolbar || linkPop)) updateFloating(handle.view);
  }

  // ── issue hover cards ────────────────────────────────────

  let hoverIdent = $state<string | null>(null);
  let hoverAnchor = $state<HTMLElement | null>(null);
  let hoverShow: ReturnType<typeof setTimeout> | null = null;
  let hoverHide: ReturnType<typeof setTimeout> | null = null;

  function onHostOver(e: MouseEvent) {
    const ref = (e.target as Element | null)?.closest<HTMLElement>(".cm-md-ref[data-kind='issue']");
    if (!ref) return;
    if (hoverHide) clearTimeout(hoverHide);
    if (hoverShow) clearTimeout(hoverShow);
    hoverShow = setTimeout(() => {
      hoverAnchor = ref;
      hoverIdent = ref.dataset.ident ?? null;
    }, 400);
  }

  function scheduleHoverHide() {
    if (hoverShow) clearTimeout(hoverShow);
    hoverShow = null;
    if (hoverHide) clearTimeout(hoverHide);
    hoverHide = setTimeout(() => {
      hoverIdent = null;
      hoverAnchor = null;
    }, 200);
  }

  function onHostOut(e: MouseEvent) {
    const from = (e.target as Element | null)?.closest(".cm-md-ref");
    const to = (e.relatedTarget as Element | null)?.closest?.(".cm-md-ref");
    if (from && from !== to) scheduleHoverHide();
  }

  // ── lifecycle ────────────────────────────────────────────

  $effect(() => {
    const parent = host;
    if (!parent) return;
    let cancelled = false;
    loadEditor()
      .then((m) => {
        if (cancelled || destroyed) return;
        mod = m;
        const h = m.createMarkdownEditor(parent, {
          doc: initial,
          placeholder,
          label,
          services: { imageSrc, openImage, renderBlock },
          hooks: {
            onChange: (changes, update) =>
              onChange?.(changes, update.transactions.some((tr) => tr.isUserEvent("input.task"))),
            onSelection: updateFloating,
            onFocus: (f) => {
              focused = f;
              if (!f) {
                toolbar = null;
                linkPop = null;
              }
              onFocusChange?.(f);
            },
            onSaveNow: () => onSaveNow?.(),
            onEscape: () => onEscape?.(),
            onPaste: (e) => {
              attach.handlePaste(e);
              return e.defaultPrevented;
            },
            onKeydown: (e) => attach.handleKeydown(e),
            onDropFiles: (files) => attach.enqueue(files, "drop"),
            onAttach: () => attach.openFilePicker(),
            onOpenLink: (target, kind) => openLink(target, kind, true),
          },
        });
        handle = h;
        onReady?.(h, m);
        for (const snippet of pendingInserts.splice(0)) h.insertSnippet(snippet);
        if (pendingFocus) h.focus(pendingFocus);
        pendingFocus = null;
      })
      .catch(() => {
        if (!cancelled) loadError = true;
      });
    return () => {
      cancelled = true;
    };
  });

  onDestroy(() => {
    destroyed = true;
    attach.destroy();
    handle?.destroy();
    if (hoverShow) clearTimeout(hoverShow);
    if (hoverHide) clearTimeout(hoverHide);
  });

  /** Focus the editor; "end" also moves the caret to the end. */
  export function focus(where: "end" | "keep" = "keep") {
    if (handle) handle.focus(where);
    else pendingFocus = where;
  }

  /** Insert a Markdown snippet at the caret (or at the end when the editor
   *  has never had one). */
  export function insert(snippet: string) {
    if (!handle) {
      pendingInserts.push(snippet);
      pendingFocus = "keep";
      return;
    }
    if (!handle.view.hasFocus && handle.view.state.selection.main.head === 0 && handle.text()) {
      handle.focus("end");
    }
    attach.insert(snippet);
  }

  export function getHandle() {
    return handle;
  }
</script>

<svelte:window onscroll={onWindowScroll} onresize={onWindowScroll} />

<AttachComposer composer={attach} overlay={false} {footer}>
  <!-- svelte-ignore a11y_no_static_element_interactions, a11y_mouse_events_have_key_events -->
  <div
    class="md-editor"
    class:md-editor--loading={!handle}
    onmouseover={onHostOver}
    onmouseout={onHostOut}
  >
    <div bind:this={host}></div>
    {#if !handle}
      {#if initial.trim()}
        <Markdown content={initial} class={fallbackClass} />
      {:else}
        <p class="md-editor__placeholder">{placeholder}</p>
      {/if}
    {/if}
  </div>
</AttachComposer>

{#snippet footer()}
  <!-- Pressing a footer control must not take focus from the text, or the
       footer would hide before the click lands. -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    class="md-editor__footer"
    class:md-editor__footer--shown={focused || footerVisible || attach.uploads.busy}
    onmousedown={(e) => {
      if (!(e.target as Element).closest("input, textarea")) e.preventDefault();
    }}
  >
    <AttachTrigger
      variant="plain"
      busy={attach.uploads.busy}
      onFiles={(files, source) => attach.enqueue(files, source)}
    />
    <span class="md-editor__hint">
      <kbd>/</kbd> for blocks · <kbd>#</kbd> to reference an issue
    </span>
    {#if status}{@render status({ focused })}{/if}
  </div>
{/snippet}

{#if loadError}
  <p class="md-editor__error">The editor didn't load. Reload the page to edit this text.</p>
{/if}

{#if toolbar}
  <FormatToolbar
    x={toolbar.x}
    y={toolbar.y}
    placement={coarse ? "below" : "above"}
    active={toolbar.active}
    onAction={runFormat}
    onQuote={onQuote ? quoteSelection : undefined}
  />
{/if}

{#if linkPop}
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    class="md-link-pop"
    style="left: {linkPop.x}px; top: {linkPop.y}px;"
    onmousedown={(e) => e.preventDefault()}
  >
    <span class="md-link-pop__href">{linkPop.link.href}</span>
    <button
      type="button"
      class="md-link-pop__open"
      onclick={() => linkPop && openLink(linkPop.link.href, linkPop.link.kind)}
    >
      <ExternalLink size={13} />
      Open
    </button>
  </div>
{/if}

{#if hoverIdent && hoverAnchor}
  <IssueHoverCard
    identifier={hoverIdent}
    anchorEl={hoverAnchor}
    onEnter={() => hoverHide && clearTimeout(hoverHide)}
    onLeave={scheduleHoverHide}
  />
{/if}

{#if lightbox}
  <ImageLightbox images={lightbox.images} start={lightbox.start} onClose={() => (lightbox = null)} />
{/if}

<style>
  .md-editor {
    min-height: var(--md-editor-min-height, 3.4em);
    cursor: text;
  }
  .md-editor--loading {
    cursor: progress;
  }
  .md-editor__placeholder {
    margin: 0;
    font-size: 0.875rem;
    line-height: 1.7;
    font-style: italic;
    color: var(--text-faint);
  }
  /* Hidden, not removed, while the text is not focused: the attach
     trigger's file input has to survive the blur a file dialog causes. */
  .md-editor__footer {
    display: none;
    align-items: center;
    gap: 0.75rem;
    min-height: 2rem;
    margin-top: 0.5rem;
    padding-top: 0.375rem;
    border-top: 1px solid var(--border);
    animation: md-footer-in 0.15s var(--ease-out-expo);
  }
  .md-editor__footer--shown,
  .md-editor__footer:focus-within {
    display: flex;
  }
  @keyframes md-footer-in {
    from {
      opacity: 0;
    }
  }
  .md-editor__hint {
    font-size: 0.75rem;
    color: var(--text-faint);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .md-editor__hint kbd {
    font-family: var(--font-mono);
    font-size: 0.6875rem;
    padding: 0.05rem 0.3rem;
    border: 1px solid var(--border);
    border-radius: 0.25rem;
    background: var(--bg-subtle);
  }
  @media (hover: none) {
    .md-editor__hint {
      display: none;
    }
  }
  .md-editor__error {
    margin: 0.5rem 0 0;
    font-size: 0.8125rem;
    color: var(--error);
  }
  .md-link-pop {
    position: fixed;
    z-index: 60;
    transform: translateY(6px);
    display: flex;
    align-items: center;
    gap: 0.5rem;
    max-width: min(26rem, calc(100vw - 1rem));
    padding: 0.25rem 0.25rem 0.25rem 0.625rem;
    border-radius: 0.5rem;
    background: var(--surface);
    border: 1px solid var(--border);
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.12);
    font-size: 0.75rem;
  }
  .md-link-pop__href {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--text-muted);
    font-family: var(--font-mono);
  }
  .md-link-pop__open {
    display: inline-flex;
    align-items: center;
    gap: 0.3125rem;
    flex-shrink: 0;
    padding: 0.3125rem 0.5rem;
    border: 0;
    border-radius: 0.375rem;
    background: var(--bg-subtle);
    color: var(--text);
    font-weight: 500;
    cursor: pointer;
  }
  .md-link-pop__open:hover {
    background: var(--accent-subtle);
    color: var(--accent);
  }
  @media (prefers-reduced-motion: reduce) {
    .md-editor__footer {
      animation: none;
    }
  }
</style>

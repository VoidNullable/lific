<script lang="ts">
  import {
    getPage,
    updatePage,
    deletePage,
    type Page,
  } from "../lib/api";
  import Markdown from "../lib/Markdown.svelte";
  import { ArrowLeft, Ellipsis, Trash2 } from "lucide-svelte";

  let {
    navigate,
    projectIdentifier,
    pageId,
    editable = true,
  }: {
    navigate: (path: string) => void;
    projectIdentifier: string;
    pageId: number;
    editable?: boolean;
  } = $props();

  let page = $state<Page | null>(null);
  let loading = $state(true);
  let error = $state("");

  // Editing
  let editingTitle = $state(false);
  let editingContent = $state(false);
  let draftTitle = $state("");
  let draftContent = $state("");
  let contentEl = $state<HTMLTextAreaElement | null>(null);
  let contentWrapperEl = $state<HTMLElement | null>(null);
  let preEditHeight = $state<number | null>(null);

  // Save
  let saving = $state(false);
  let lastSaved = $state<string | null>(null);

  // Delete
  let menuOpen = $state(false);
  let confirmingDelete = $state(false);
  let deleting = $state(false);

  $effect(() => {
    const id = pageId;
    loadPage(id);
  });

  async function loadPage(id: number) {
    loading = true;
    error = "";
    const res = await getPage(id);
    if (!res.ok) { error = res.error; loading = false; return; }
    page = res.data;
    loading = false;
  }

  function handleWindowClick() {
    menuOpen = false;
    confirmingDelete = false;
  }

  // ── Save ─────────────────────────────────────────────

  async function saveField(field: string, value: unknown) {
    if (!page) return;
    saving = true;
    const res = await updatePage(page.id, { [field]: value });
    if (res.ok) {
      page = res.data;
      lastSaved = new Date().toLocaleTimeString([], {
        hour: "2-digit",
        minute: "2-digit",
      });
    }
    saving = false;
  }

  // ── Delete ───────────────────────────────────────────

  async function confirmDelete() {
    if (!page) return;
    deleting = true;
    const res = await deletePage(page.id);
    if (res.ok) {
      navigate(`/${projectIdentifier}/pages`);
    } else {
      deleting = false;
      confirmingDelete = false;
    }
  }

  // ── Title ────────────────────────────────────────────

  function startEditTitle() {
    if (!editable || !page) return;
    draftTitle = page.title;
    editingTitle = true;
  }

  async function commitTitle() {
    if (!page) return;
    editingTitle = false;
    const trimmed = draftTitle.trim();
    if (trimmed && trimmed !== page.title) {
      await saveField("title", trimmed);
    }
  }

  // ── Content ──────────────────────────────────────────

  function startEditContent() {
    if (!editable || !page) return;
    if (contentWrapperEl) preEditHeight = contentWrapperEl.offsetHeight;
    draftContent = page.content;
    editingContent = true;
    requestAnimationFrame(() => autoResize());
  }

  async function commitContent() {
    if (!page) return;
    editingContent = false;
    preEditHeight = null;
    if (draftContent !== page.content) {
      await saveField("content", draftContent);
    }
  }

  function cancelContent() {
    editingContent = false;
    preEditHeight = null;
  }

  function autoResize() {
    const el = contentEl;
    if (!el) return;
    el.style.height = "0";
    const floor = preEditHeight ?? 0;
    el.style.height = Math.max(el.scrollHeight, floor) + "px";
  }

  function formatDate(iso: string): string {
    const d = new Date(iso + "Z");
    return d.toLocaleDateString("en-US", {
      month: "short",
      day: "numeric",
      year: "numeric",
      hour: "numeric",
      minute: "2-digit",
    });
  }
</script>

<svelte:window onclick={handleWindowClick} />

{#if loading}
  <div class="h-full flex items-center justify-center">
    <div
      class="size-6 rounded-full border-2 border-[var(--border)]
             border-t-[var(--accent)] animate-spin"
    ></div>
  </div>
{:else if error}
  <div class="h-full flex flex-col items-center justify-center gap-3">
    <p class="text-[var(--error)] text-[0.875rem]">{error}</p>
    <button
      class="text-[0.8125rem] text-[var(--accent)] hover:underline"
      onclick={() => navigate(`/${projectIdentifier}/pages`)}
    >
      Back to pages
    </button>
  </div>
{:else if page}
  <div class="h-full flex flex-col">
    <!-- Top bar -->
    <div
      class="shrink-0 flex items-center gap-3 px-6 py-2.5
             border-b border-[var(--border)] bg-[var(--surface)]"
    >
      <button
        class="flex items-center gap-1.5 text-[0.8125rem] text-[var(--text-muted)]
               hover:text-[var(--text)] transition-colors rounded px-1.5 py-0.5
               hover:bg-[var(--bg-subtle)]"
        onclick={() => navigate(`/${projectIdentifier}/pages`)}
      >
        <ArrowLeft size={14} />
        Pages
      </button>

      <span class="text-[var(--text-faint)]">/</span>
      <span class="text-[0.8125rem] font-mono text-[var(--text-muted)]">
        {page.identifier}
      </span>

      <!-- Save indicator + menu -->
      <div class="ml-auto flex items-center gap-2">
        <span class="text-[0.75rem] text-[var(--text-faint)]">
          {#if saving}
            <span class="animate-pulse">Saving...</span>
          {:else if lastSaved}
            Saved at {lastSaved}
          {/if}
        </span>

        {#if editable}
          <div class="relative">
            <button
              class="size-7 flex items-center justify-center rounded-md
                     text-[var(--text-faint)] hover:text-[var(--text)]
                     hover:bg-[var(--bg-subtle)] transition-colors"
              onclick={(e) => {
                e.stopPropagation();
                if (confirmingDelete) { confirmingDelete = false; menuOpen = false; }
                else { menuOpen = !menuOpen; }
              }}
            >
              <Ellipsis size={14} />
            </button>

            {#if menuOpen && !confirmingDelete}
              <!-- svelte-ignore a11y_no_static_element_interactions a11y_click_events_have_key_events -->
              <div
                class="absolute right-0 top-full mt-1 z-20 w-[180px]
                       bg-[var(--surface)] border border-[var(--border)]
                       rounded-md shadow-lg py-1"
                onclick={(e) => e.stopPropagation()}
              >
                <button
                  class="w-full flex items-center gap-2 px-3 py-1.5 text-left
                         text-[0.8125rem] text-[var(--error)]
                         hover:bg-[var(--error-bg)] transition-colors"
                  onclick={() => { confirmingDelete = true; }}
                >
                  <Trash2 size={14} />
                  Delete page
                </button>
              </div>
            {/if}

            {#if confirmingDelete}
              <!-- svelte-ignore a11y_no_static_element_interactions a11y_click_events_have_key_events -->
              <div
                class="absolute right-0 top-full mt-1 z-20 w-[240px]
                       bg-[var(--surface)] border border-[var(--border)]
                       rounded-md shadow-lg p-3"
                onclick={(e) => e.stopPropagation()}
              >
                <p class="text-[0.8125rem] text-[var(--text)] mb-1 font-medium">
                  Delete {page.identifier}?
                </p>
                <p class="text-[0.75rem] text-[var(--text-muted)] mb-3">
                  This can't be undone.
                </p>
                <div class="flex items-center gap-2">
                  <button
                    class="text-[0.8125rem] font-medium text-white
                           bg-[var(--error)] px-3 py-1.5 rounded-md
                           hover:opacity-90 transition-opacity
                           disabled:opacity-50"
                    disabled={deleting}
                    onclick={confirmDelete}
                  >
                    {deleting ? "Deleting..." : "Delete"}
                  </button>
                  <button
                    class="text-[0.8125rem] text-[var(--text-muted)] px-3 py-1.5
                           rounded-md hover:bg-[var(--bg-subtle)] transition-colors"
                    onclick={() => { confirmingDelete = false; menuOpen = false; }}
                  >
                    Cancel
                  </button>
                </div>
              </div>
            {/if}
          </div>
        {/if}
      </div>
    </div>

    <!-- Content -->
    <div class="flex-1 overflow-y-auto">
      <div class="max-w-[720px] mx-auto px-8 py-8">
        <!-- Title -->
        {#if editingTitle}
          <!-- svelte-ignore a11y_autofocus -->
          <input
            type="text"
            bind:value={draftTitle}
            class="w-full text-[1.75rem] font-display tracking-tight
                   bg-transparent border-none outline-none
                   text-[var(--text)] py-1 mb-6"
            onblur={commitTitle}
            onkeydown={(e) => {
              if (e.key === "Enter") commitTitle();
              if (e.key === "Escape") { editingTitle = false; }
            }}
            autofocus
          />
        {:else}
          <!-- svelte-ignore a11y_no_static_element_interactions a11y_no_noninteractive_element_interactions -->
          <h1
            class="text-[1.75rem] font-display tracking-tight text-[var(--text)]
                   py-1 mb-6 rounded transition-colors
                   {editable ? 'cursor-text hover:bg-[var(--bg-subtle)]' : ''}"
            onclick={startEditTitle}
          >
            {page.title}
          </h1>
        {/if}

        <!-- Body content -->
        <section
          bind:this={contentWrapperEl}
          style={preEditHeight != null ? `min-height: ${preEditHeight}px;` : ""}
        >
          {#if editingContent}
            <!-- svelte-ignore a11y_autofocus -->
            <textarea
              bind:value={draftContent}
              bind:this={contentEl}
              class="w-full text-[0.875rem] leading-[1.7] text-[var(--text)]
                     bg-transparent border-none outline-none resize-none
                     p-0 m-0 font-[var(--font-body)]"
              style={preEditHeight != null ? `height: ${preEditHeight}px;` : ""}
              placeholder="Start writing... (markdown supported)"
              onkeydown={(e) => { if (e.key === "Escape") cancelContent(); }}
              oninput={autoResize}
              autofocus
            ></textarea>
            <div class="flex items-center gap-2 mt-3 pt-3 border-t border-[var(--border)]">
              <button
                class="text-[0.8125rem] font-medium text-[var(--accent-text)]
                       bg-[var(--accent)] px-3 py-1.5 rounded-md
                       hover:bg-[var(--accent-hover)] transition-colors"
                onclick={commitContent}
              >
                Save
              </button>
              <button
                class="text-[0.8125rem] text-[var(--text-muted)] px-3 py-1.5
                       rounded-md hover:bg-[var(--bg-subtle)] transition-colors"
                onclick={cancelContent}
              >
                Cancel
              </button>
              <span class="text-[0.75rem] text-[var(--text-faint)] ml-auto">
                Markdown · Esc to cancel
              </span>
            </div>
          {:else}
            <!-- svelte-ignore a11y_no_static_element_interactions -->
            <div
              class="transition-colors min-h-[120px]
                     {editable ? 'cursor-text hover:bg-[var(--bg-subtle)] rounded-md' : ''}"
              onclick={startEditContent}
            >
              {#if page.content.trim()}
                <Markdown content={page.content} />
              {:else}
                <p class="text-[0.875rem] text-[var(--text-faint)] italic py-2">
                  {editable ? "Click to start writing..." : "Empty page"}
                </p>
              {/if}
            </div>
          {/if}
        </section>

        <!-- Meta -->
        <div class="mt-10 pt-6 border-t border-[var(--border)] flex gap-8">
          <div>
            <span class="block text-[0.6875rem] font-semibold uppercase tracking-widest text-[var(--text-faint)] mb-0.5">
              Created
            </span>
            <span class="text-[0.8125rem] text-[var(--text-muted)]">
              {formatDate(page.created_at)}
            </span>
          </div>
          <div>
            <span class="block text-[0.6875rem] font-semibold uppercase tracking-widest text-[var(--text-faint)] mb-0.5">
              Updated
            </span>
            <span class="text-[0.8125rem] text-[var(--text-muted)]">
              {formatDate(page.updated_at)}
            </span>
          </div>
        </div>
      </div>
    </div>
  </div>
{/if}

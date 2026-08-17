<script lang="ts">
  // LIF-262: compact "Attachments (n)" section for issue/page detail views.
  // Lists the attachments linked to an entity: image thumbnails (click to open
  // the lightbox) and non-image chips (file icon + name + human size +
  // download). Read-only surface — attaching happens through the description
  // editor / comment composer; this just surfaces what's linked.

  import {
    listEntityAttachments,
    attachmentThumbnailUrl,
    type Attachment,
    type AttachmentEntity,
  } from "./api";
  import { Paperclip } from "lucide-svelte";
  import AttachmentView from "./attachments/viewers/AttachmentView.svelte"; // LIF-418

  let {
    entityType,
    entityId,
    // Bump this to force a re-fetch after the body/comments change (a new
    // markdown reference may have just been linked server-side).
    refreshKey = 0,
  }: {
    entityType: AttachmentEntity;
    entityId: number;
    refreshKey?: number;
  } = $props();

  let attachments = $state<Attachment[]>([]);
  let loaded = $state(false);
  let lightboxSrc = $state<string | null>(null);
  let lightboxAlt = $state("");

  async function load() {
    const res = await listEntityAttachments(entityType, entityId);
    if (res.ok) {
      attachments = res.data;
    }
    loaded = true;
  }

  $effect(() => {
    // Re-run on entity change or refreshKey bump.
    entityId;
    refreshKey;
    void load();
  });

  function urlFor(id: number): string {
    return `/api/attachments/${id}`;
  }

  function isImage(a: Attachment): boolean {
    return a.mime.startsWith("image/");
  }

  // LIF-418: images keep the tile-and-lightbox treatment they have always had
  // (a wrapping row of thumbnails reads better here than a stack of cards);
  // everything else goes through the shared dispatcher, which turns a log, a
  // patch, a CSV, a zip or a screen recording into a real inline viewer and
  // falls back to the same download chip for anything it does not know.
  let images = $derived(attachments.filter(isImage));
  let others = $derived(attachments.filter((a) => !isImage(a)));

  /** Thumbnail src for a tile, with the full asset as the error fallback. */
  function onThumbError(event: Event, a: Attachment) {
    const img = event.currentTarget as HTMLImageElement;
    if (img.dataset.fullSrc === "true") return;
    img.dataset.fullSrc = "true";
    img.src = urlFor(a.id);
  }
</script>

{#if loaded && attachments.length > 0}
  <section class="att">
    <header class="att__head">
      <Paperclip size={14} />
      <h3 class="att__title">Attachments</h3>
      <span class="att__count">{attachments.length}</span>
    </header>

    {#if images.length > 0}
      <div class="att__grid">
        {#each images as a (a.id)}
          <button
            type="button"
            class="att__thumb"
            title={a.filename}
            onclick={() => {
              lightboxSrc = urlFor(a.id);
              lightboxAlt = a.alt_text ?? a.filename;
            }}
          >
            <img
              src={attachmentThumbnailUrl(a.id)}
              alt={a.alt_text ?? a.filename}
              loading="lazy"
              onerror={(e) => onThumbError(e, a)}
            />
            <span class="att__thumb-name">{a.filename}</span>
          </button>
        {/each}
      </div>
    {/if}

    {#if others.length > 0}
      <div class="att__stack">
        {#each others as a (a.id)}
          <AttachmentView
            id={a.id}
            filename={a.filename}
            mime={a.mime}
            sizeBytes={a.size_bytes}
            altText={a.alt_text ?? null}
            hasThumbnail={a.has_thumbnail ?? false}
          />
        {/each}
      </div>
    {/if}
  </section>
{/if}

<svelte:window
  onkeydown={(e) => {
    if (e.key === "Escape" && lightboxSrc) lightboxSrc = null;
  }}
/>

{#if lightboxSrc}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    class="att__lightbox"
    role="dialog"
    aria-modal="true"
    aria-label="Image preview"
    tabindex="-1"
    onclick={() => (lightboxSrc = null)}
  >
    <img src={lightboxSrc} alt={lightboxAlt} />
  </div>
{/if}

<style>
  .att {
    margin-top: 1.75rem;
    padding-top: 1.5rem;
    border-top: 1px solid var(--border);
  }
  .att__head {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    margin-bottom: 1rem;
    color: var(--text-muted);
  }
  .att__title {
    font-size: 0.9375rem;
    font-weight: 600;
    color: var(--text);
    margin: 0;
  }
  .att__count {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    min-width: 1.25rem;
    height: 1.25rem;
    padding: 0 0.375rem;
    border-radius: 999px;
    background: var(--bg-subtle);
    color: var(--text-muted);
    font-size: 0.6875rem;
    font-weight: 600;
    font-variant-numeric: tabular-nums;
  }

  .att__grid {
    display: flex;
    flex-wrap: wrap;
    gap: 0.75rem;
  }

  /* Non-image attachments render as full-width viewer cards, so they stack
     rather than share the image row's wrapping flow. */
  .att__stack {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    margin-top: 0.75rem;
  }
  .att__stack:first-child {
    margin-top: 0;
  }

  /* Image thumbnail tile. */
  .att__thumb {
    display: flex;
    flex-direction: column;
    width: 8rem;
    padding: 0;
    border: 1px solid var(--border);
    border-radius: 0.5rem;
    background: var(--surface);
    overflow: hidden;
    cursor: zoom-in;
    transition: border-color 0.15s var(--ease-out-expo);
  }
  .att__thumb:hover {
    border-color: var(--accent);
  }
  .att__thumb img {
    width: 100%;
    height: 5.5rem;
    object-fit: cover;
    display: block;
    background: var(--bg-subtle);
  }
  .att__thumb-name {
    padding: 0.3125rem 0.5rem;
    font-size: 0.6875rem;
    color: var(--text-muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    text-align: left;
  }

  .att__lightbox {
    position: fixed;
    inset: 0;
    z-index: 1200;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 2rem;
    background: rgba(0, 0, 0, 0.78);
    cursor: zoom-out;
  }
  .att__lightbox img {
    max-width: 100%;
    max-height: 100%;
    border-radius: 0.5rem;
    box-shadow: 0 24px 64px rgba(0, 0, 0, 0.5);
  }

  @media (prefers-reduced-motion: reduce) {
    .att__thumb {
      transition: none;
    }
  }
</style>

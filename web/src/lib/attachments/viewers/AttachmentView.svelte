<script lang="ts">
  // LIF-418: the one place that decides how an attachment renders.
  //
  // Both surfaces that show attachments go through here: the markdown body
  // (Markdown.svelte swaps each attachment link for one of these) and the
  // "Attachments (n)" section on issue and page detail. Anything this
  // dispatcher cannot place lands on FileChip, which is the download chip the
  // app has always shown, so an unknown type, a missing preview endpoint, or a
  // backend older than the frontend all degrade to the previous behavior
  // rather than to an error.

  import { viewerKindFor, MAX_INLINE_BYTES, INLINE_FETCH_KINDS } from "./kind";
  import { attachmentThumbnailUrl, attachmentUrl } from "../../api";
  import FileChip from "./FileChip.svelte";
  import TextViewer from "./TextViewer.svelte";
  import DiffViewer from "./DiffViewer.svelte";
  import DataViewer from "./DataViewer.svelte";
  import ZipViewer from "./ZipViewer.svelte";
  import SqliteViewer from "./SqliteViewer.svelte";
  import MediaViewer from "./MediaViewer.svelte";

  let {
    id,
    filename,
    /** Stored mime when the caller has the attachment record. Markdown links
     *  carry only an id and a label, so this is often null and dispatch falls
     *  back to the extension. */
    mime = null,
    sizeBytes = null,
    altText = null,
    hasThumbnail = false,
  }: {
    id: number;
    filename: string;
    mime?: string | null;
    sizeBytes?: number | null;
    altText?: string | null;
    hasThumbnail?: boolean;
  } = $props();

  const kind = $derived(viewerKindFor({ filename, mime }));

  /** Viewers that pull the file down refuse anything past the inline cap. The
   *  cap only applies when the size is known; when it is not, the fetch itself
   *  checks the payload length. */
  const tooBig = $derived(
    sizeBytes != null && sizeBytes > MAX_INLINE_BYTES && INLINE_FETCH_KINDS.has(kind),
  );

  // Images: the thumbnail is the inline src so the page does not download a
  // 12 MB screenshot to show a 400px-wide preview, and the lightbox always
  // opens the full asset. A 404 from the thumbnail endpoint (no thumbnail was
  // generated, or the backend predates it) swaps the src for the full image.
  let usedFallback = $state(false);
  let lightboxOpen = $state(false);
  const imageSrc = $derived(
    usedFallback ? attachmentUrl(id) : attachmentThumbnailUrl(id),
  );

  function onImageError() {
    usedFallback = true;
  }
</script>

{#if tooBig}
  <FileChip {id} {filename} {sizeBytes} note="Too large to preview inline" />
{:else if kind === "image"}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <img
    class="av__image"
    src={imageSrc}
    alt={altText ?? filename}
    loading="lazy"
    onerror={onImageError}
    onclick={() => (lightboxOpen = true)}
  />
  {#if lightboxOpen}
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
      class="av__lightbox"
      role="dialog"
      aria-modal="true"
      aria-label="Image preview"
      tabindex="-1"
      onclick={() => (lightboxOpen = false)}
    >
      <img src={attachmentUrl(id)} alt={altText ?? filename} />
    </div>
  {/if}
{:else if kind === "video" || kind === "audio"}
  <MediaViewer {id} {filename} {mime} {sizeBytes} {hasThumbnail} {kind} />
{:else if kind === "diff"}
  <DiffViewer {id} {filename} {sizeBytes} />
{:else if kind === "csv"}
  <DataViewer {id} {filename} {sizeBytes} mode="table" />
{:else if kind === "json"}
  <DataViewer {id} {filename} {sizeBytes} mode="json" />
{:else if kind === "zip"}
  <ZipViewer {id} {filename} {sizeBytes} />
{:else if kind === "sqlite"}
  <SqliteViewer {id} {filename} {sizeBytes} />
{:else if kind === "text"}
  <TextViewer {id} {filename} {sizeBytes} />
{:else}
  <FileChip {id} {filename} {sizeBytes} />
{/if}

<svelte:window
  onkeydown={(e) => {
    if (e.key === "Escape" && lightboxOpen) lightboxOpen = false;
  }}
/>

<style>
  .av__image {
    display: block;
    max-width: 100%;
    max-height: 32rem;
    height: auto;
    margin: 0.5rem 0;
    border: 1px solid var(--border);
    border-radius: 0.5rem;
    cursor: zoom-in;
    transition: filter 0.15s var(--ease-out-expo);
  }
  .av__image:hover {
    filter: brightness(0.96);
  }
  .av__lightbox {
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
  .av__lightbox img {
    max-width: 100%;
    max-height: 100%;
    border-radius: 0.5rem;
    box-shadow: 0 24px 64px rgba(0, 0, 0, 0.5);
  }
  @media (prefers-reduced-motion: reduce) {
    .av__image {
      transition: none;
    }
  }
</style>

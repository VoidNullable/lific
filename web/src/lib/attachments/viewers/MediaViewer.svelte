<script lang="ts">
  // LIF-418: inline players for attached video and audio.
  //
  // No custom transport controls: the native ones are keyboard accessible, get
  // picture-in-picture and playback speed for free, and already look native in
  // both themes. `preload="metadata"` means opening an issue with a screen
  // recording on it costs a few kilobytes, not the whole file; the browser
  // then range-requests as you scrub (the backend serves Range for these
  // types, which is what makes seeking work at all).
  //
  // A codec the browser refuses fires `error` on the element, and the card
  // swaps itself for the download chip rather than leaving a dead black box.

  import { attachmentThumbnailUrl, attachmentUrl } from "../../api";
  import FileChip from "./FileChip.svelte";

  let {
    id,
    filename,
    mime = null,
    sizeBytes = null,
    hasThumbnail = false,
    kind,
  }: {
    id: number;
    filename: string;
    mime?: string | null;
    sizeBytes?: number | null;
    hasThumbnail?: boolean;
    kind: "video" | "audio";
  } = $props();

  let failed = $state(false);

  const src = $derived(attachmentUrl(id));
  const poster = $derived(hasThumbnail ? attachmentThumbnailUrl(id) : undefined);
  // Hand the browser the exact mime when we have it, so it can reject an
  // unsupported codec up front instead of downloading first.
  const type = $derived(mime ?? undefined);
</script>

{#if failed}
  <FileChip {id} {filename} {sizeBytes} note="Playback not supported in this browser" />
{:else}
  <figure class="mv" class:mv--audio={kind === "audio"}>
    {#if kind === "video"}
      <!-- svelte-ignore a11y_media_has_caption -->
      <video
        class="mv__video"
        controls
        preload="metadata"
        {poster}
        onerror={() => (failed = true)}
      >
        <source {src} {type} />
      </video>
    {:else}
      <audio class="mv__audio" controls preload="metadata" onerror={() => (failed = true)}>
        <source {src} {type} />
      </audio>
    {/if}
    <figcaption class="mv__caption" title={filename}>{filename}</figcaption>
  </figure>
{/if}

<style>
  .mv {
    display: block;
    margin: 0.75rem 0;
    padding: 0;
    border: 1px solid var(--border);
    border-radius: 0.5rem;
    background: var(--surface);
    overflow: hidden;
    max-width: 100%;
    width: fit-content;
  }
  .mv--audio {
    width: 100%;
    max-width: 28rem;
  }
  .mv__video {
    display: block;
    max-width: 100%;
    /* Capped so a 4K screen recording does not take over the column. */
    max-height: 26rem;
    background: #000;
  }
  .mv__audio {
    display: block;
    width: 100%;
    padding: 0.5rem 0.5rem 0.25rem;
  }
  .mv__caption {
    padding: 0.3125rem 0.625rem;
    border-top: 1px solid var(--border);
    background: var(--bg-subtle);
    color: var(--text-muted);
    font-size: var(--text-micro);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
</style>

<script lang="ts">
  // LIF-262: compact "Attachments (n)" section for issue/page detail views.
  // Lists the attachments linked to an entity: image thumbnails (click to open
  // the lightbox) and non-image chips (file icon + name + human size +
  // download). Attaching still happens through the description editor and the
  // comment composer; this surfaces what's linked.
  //
  // LIF-418: it can also delete. The button appears only for the uploader, a
  // project maintainer, or an admin, mirroring the server's gate, and the
  // confirm is inline rather than a modal. It states how many references go
  // with the file, because deleting a screenshot three issues embed breaks
  // all three and this view only shows one of them.

  import {
    listEntityAttachments,
    downloadAttachment,
    deleteAttachment,
    getAttachmentLinks,
    updateAttachmentAlt,
    attachmentThumbnailUrl,
    attachmentUrl,
    formatBytes,
    me,
    type Attachment,
    type AttachmentEntity,
    type AttachmentLinks,
    type LinkedEntity,
  } from "./api";
  import { inPublicScope } from "./publicScope"; // LIF-471
  import {
    canDeleteAttachment,
    deleteConfirmMessage,
    entityChipLabel,
    entityHref,
  } from "./files/files";
  import { projectCodeOf } from "./references";
  import { projectRole } from "./projectRole.svelte";
  import { toast } from "./toast/toast.svelte";
  import { Info, Paperclip, TextCursorInput, Trash2, X } from "lucide-svelte";
  import { markdownFor } from "./attachments/compose";
  import AttachmentView from "./attachments/viewers/AttachmentView.svelte"; // LIF-418

  let {
    entityType,
    entityId,
    // Bump this to force a re-fetch after the body/comments change (a new
    // markdown reference may have just been linked server-side).
    refreshKey = 0,
    // When set, each file offers Insert, which hands its Markdown to the
    // body editor (opening it if needed).
    onInsert,
  }: {
    entityType: AttachmentEntity;
    entityId: number;
    refreshKey?: number;
    onInsert?: (snippet: string) => void;
  } = $props();

  function insert(a: Attachment) {
    onInsert?.(
      markdownFor({ mime: a.mime, filename: a.filename, url: `/api/attachments/${a.id}` }, a.alt_text),
    );
  }

  let attachments = $state<Attachment[]>([]);
  let loaded = $state(false);
  let lightboxSrc = $state<string | null>(null);
  let lightboxAlt = $state("");

  // Delete flow: which row is confirming, how many references it has, and
  // whether a request is in flight.
  let confirmingId = $state<number | null>(null);
  let confirmingRefs = $state(1);
  let deletingId = $state<number | null>(null);
  let viewerId = $state<number | null>(null);

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
    confirmingId = null;
    void load();
  });

  $effect(() => {
    if (viewerId === null) {
      void me().then((res) => {
        if (res.ok) viewerId = res.data.id;
      });
    }
  });

  function urlFor(id: number): string {
    return attachmentUrl(id);
  }

  function isImage(a: Attachment): boolean {
    return a.mime.startsWith("image/");
  }

  function mayDelete(a: Attachment): boolean {
    // LIF-471: `canDeleteAttachment` fails open while the viewer is unknown,
    // which is right for a signed-in page still loading `me()` and wrong for
    // a public page, where the viewer is unknown by design.
    if (inPublicScope()) return false;
    return canDeleteAttachment({
      uploaderId: a.uploader_id,
      viewerId,
      isAdmin: projectRole.isAdmin,
      canEdit: projectRole.canEdit,
    });
  }

  /** Open the inline confirm, asking the server how many entities reference
   *  the file so the sentence is accurate. The where-used endpoint belongs to
   *  a sibling workstream; when it is not there, this view knows of exactly
   *  one reference (its own entity), which is the honest floor. */
  async function startDelete(a: Attachment) {
    if (confirmingId === a.id) {
      confirmingId = null;
      return;
    }
    confirmingId = a.id;
    confirmingRefs = 1;
    const res = await getAttachmentLinks(a.id);
    if (res.ok && confirmingId === a.id) {
      confirmingRefs = res.data.entities.length;
    }
  }

  async function confirmDelete(a: Attachment) {
    deletingId = a.id;
    const res = await deleteAttachment(a.id);
    deletingId = null;
    confirmingId = null;
    if (!res.ok) {
      toast(`Couldn't delete ${a.filename}: ${res.error}`, { kind: "error" });
      return;
    }
    toast(`Deleted ${a.filename}.`, { kind: "success" });
    await load();
  }

  // LIF-418: images keep the tile-and-lightbox treatment they have always had
  // (a wrapping row of thumbnails reads better here than a stack of cards);
  // everything else goes through the shared dispatcher, which turns a log, a
  // patch, a CSV, a zip or a screen recording into a real inline viewer and
  // falls back to the same download chip for anything it does not know.
  let images = $derived(attachments.filter(isImage));
  let others = $derived(attachments.filter((a) => !isImage(a)));

  // Detail panel: alt text (editable under the delete gate) and where the
  // file is used. `undefined` links means still loading, `null` failed.
  let detailId = $state<number | null>(null);
  let detailLinks = $state<AttachmentLinks | null | undefined>(undefined);
  let altDraft = $state("");
  let savingAlt = $state(false);

  async function toggleDetail(a: Attachment) {
    if (detailId === a.id) {
      detailId = null;
      return;
    }
    detailId = a.id;
    detailLinks = undefined;
    altDraft = a.alt_text ?? "";
    const res = await getAttachmentLinks(a.id);
    if (detailId === a.id) detailLinks = res.ok ? res.data : null;
  }

  async function saveAlt(a: Attachment) {
    const next = altDraft.trim();
    if (next === (a.alt_text ?? "")) return;
    savingAlt = true;
    const res = await updateAttachmentAlt(a.id, next === "" ? null : next);
    savingAlt = false;
    if (!res.ok) {
      toast(`Couldn't save the alt text: ${res.error}`, { kind: "error" });
      return;
    }
    attachments = attachments.map((row) => (row.id === a.id ? { ...row, ...res.data } : row));
    altDraft = res.data.alt_text ?? "";
    toast("Alt text saved.", { kind: "success" });
  }

  function onAltKeydown(e: KeyboardEvent, a: Attachment) {
    if (e.key === "Enter") {
      e.preventDefault();
      void saveAlt(a);
    } else if (e.key === "Escape") {
      e.preventDefault();
      altDraft = a.alt_text ?? "";
    }
  }

  /** Hash route for a referencing entity, in whichever project it lives. */
  function linkedHref(entity: LinkedEntity): string | null {
    if (!entity.identifier) return null;
    const href = entityHref(projectCodeOf(entity.identifier), entity);
    return href ? `#${href}` : null;
  }

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
          <div class="att__item">
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
            {#if !inPublicScope()}
              <button
                type="button"
                class="att__info"
                title="Details for {a.filename}"
                aria-label="Details for {a.filename}"
                aria-expanded={detailId === a.id}
                onclick={() => void toggleDetail(a)}
              >
                <Info size={13} />
              </button>
            {/if}
            {#if onInsert && !inPublicScope()}
              <button
                type="button"
                class="att__insert"
                title="Insert {a.filename} into the body"
                aria-label="Insert {a.filename} into the body"
                onclick={() => insert(a)}
              >
                <TextCursorInput size={13} />
              </button>
            {/if}
            {#if mayDelete(a)}
              <button
                type="button"
                class="att__delete"
                title="Delete {a.filename}"
                aria-label="Delete {a.filename}"
                disabled={deletingId === a.id}
                onclick={() => void startDelete(a)}
              >
                <Trash2 size={13} />
              </button>
            {/if}
          </div>
        {/each}
      </div>
    {/if}

    {#if others.length > 0}
      <div class="att__stack">
        {#each others as a (a.id)}
          <div class="att__item">
            <AttachmentView
              id={a.id}
              filename={a.filename}
              mime={a.mime}
              sizeBytes={a.size_bytes}
              altText={a.alt_text ?? null}
              hasThumbnail={a.has_thumbnail ?? false}
            />
            {#if !inPublicScope()}
              <button
                type="button"
                class="att__info"
                title="Details for {a.filename}"
                aria-label="Details for {a.filename}"
                aria-expanded={detailId === a.id}
                onclick={() => void toggleDetail(a)}
              >
                <Info size={13} />
              </button>
            {/if}
            {#if onInsert && !inPublicScope()}
              <button
                type="button"
                class="att__insert"
                title="Insert {a.filename} into the body"
                aria-label="Insert {a.filename} into the body"
                onclick={() => insert(a)}
              >
                <TextCursorInput size={13} />
              </button>
            {/if}
            {#if mayDelete(a)}
              <button
                type="button"
                class="att__delete"
                title="Delete {a.filename}"
                aria-label="Delete {a.filename}"
                disabled={deletingId === a.id}
                onclick={() => void startDelete(a)}
              >
                <Trash2 size={13} />
              </button>
            {/if}
          </div>
        {/each}
      </div>
    {/if}

    {#if detailId !== null}
      {@const d = attachments.find((a) => a.id === detailId)}
      {#if d}
        <div class="att__detail" role="region" aria-label="Details for {d.filename}">
          <div class="att__detail-head">
            <span class="att__detail-name" title={d.filename}>{d.filename}</span>
            <button
              type="button"
              class="att__detail-close"
              aria-label="Close details"
              onclick={() => (detailId = null)}
            >
              <X size={13} />
            </button>
          </div>
          {#if isImage(d)}
            <div class="att__detail-row">
              <span class="att__detail-label">Alt text</span>
              {#if mayDelete(d)}
                <span class="att__detail-alt">
                  <input
                    class="att__detail-input"
                    type="text"
                    maxlength="1000"
                    placeholder="Describe this image"
                    aria-label="Alt text for {d.filename}"
                    bind:value={altDraft}
                    onkeydown={(e) => onAltKeydown(e, d)}
                  />
                  <button
                    type="button"
                    class="att__detail-save"
                    disabled={savingAlt || altDraft.trim() === (d.alt_text ?? "")}
                    onclick={() => void saveAlt(d)}
                  >
                    {savingAlt ? "Saving..." : "Save"}
                  </button>
                </span>
              {:else}
                <span class="att__detail-value">{d.alt_text ?? "None"}</span>
              {/if}
            </div>
          {/if}
          <div class="att__detail-row">
            <span class="att__detail-label">Used in</span>
            {#if detailLinks === undefined}
              <span class="att__detail-value">Loading...</span>
            {:else if detailLinks === null}
              <span class="att__detail-value">Couldn't load where this file is used.</span>
            {:else if detailLinks.entities.length === 0}
              <span class="att__detail-value">Nothing you can see references it.</span>
            {:else}
              <ul class="att__detail-uses">
                {#each detailLinks.entities as entity (entity.entity_type + entity.entity_id)}
                  {@const href = linkedHref(entity)}
                  <li>
                    {#if href}
                      <a {href} class="att__detail-link">{entityChipLabel(entity)}</a>
                    {:else}
                      <span class="att__detail-link">{entityChipLabel(entity)}</span>
                    {/if}
                    <span class="att__detail-title">{entity.title}</span>
                  </li>
                {/each}
              </ul>
            {/if}
          </div>
        </div>
      {/if}
    {/if}

    <!-- Inline confirm (never a modal): it sits under the grid, next to the
         file it is about to remove, and names the blast radius. -->
    {#if confirmingId !== null}
      {@const pending = attachments.find((a) => a.id === confirmingId)}
      {#if pending}
        <div class="att__confirm">
          <span class="att__confirm-text">
            Delete {pending.filename}? {deleteConfirmMessage(confirmingRefs)}
          </span>
          <button
            type="button"
            class="att__confirm-go"
            disabled={deletingId === pending.id}
            onclick={() => void confirmDelete(pending)}
          >
            {deletingId === pending.id ? "Deleting..." : "Delete"}
          </button>
          <button
            type="button"
            class="att__confirm-cancel"
            onclick={() => (confirmingId = null)}
          >
            Cancel
          </button>
        </div>
      {/if}
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

  /* Positioning context for the hover-revealed delete affordance. */
  .att__item {
    position: relative;
    display: flex;
  }
  .att__info {
    position: absolute;
    top: 0.25rem;
    left: 0.25rem;
    z-index: 1;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 1.375rem;
    height: 1.375rem;
    border-radius: 0.3125rem;
    background: var(--surface);
    border: 1px solid var(--border);
    color: var(--text-muted);
    opacity: 0;
    transition: opacity 0.15s var(--ease-out-expo);
  }
  .att__item:hover .att__info,
  .att__info:focus-visible,
  .att__info[aria-expanded="true"] {
    opacity: 1;
  }
  .att__info:hover {
    color: var(--accent);
  }
  @media (hover: none) {
    .att__info {
      opacity: 1;
    }
  }
  .att__insert {
    position: absolute;
    top: 0.25rem;
    left: 1.875rem;
    z-index: 1;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 1.375rem;
    height: 1.375rem;
    border-radius: 0.3125rem;
    background: var(--surface);
    border: 1px solid var(--border);
    color: var(--text-muted);
    opacity: 0;
    transition: opacity 0.15s var(--ease-out-expo);
  }
  .att__item:hover .att__insert,
  .att__insert:focus-visible {
    opacity: 1;
  }
  .att__insert:hover {
    color: var(--accent);
  }
  /* Without hover there is nothing to reveal it, so it simply shows. */
  @media (hover: none) {
    .att__insert {
      opacity: 1;
    }
  }
  .att__delete {
    position: absolute;
    top: 0.25rem;
    right: 0.25rem;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 1.375rem;
    height: 1.375rem;
    border-radius: 0.3125rem;
    background: var(--surface);
    border: 1px solid var(--border);
    color: var(--text-faint);
    opacity: 0;
    transition:
      opacity 0.15s var(--ease-out-expo),
      color 0.15s var(--ease-out-expo);
  }
  /* Keyboard users get it on focus; pointer users on hover. It never hides
     the affordance from anyone who can reach it. */
  .att__item:hover .att__delete,
  .att__delete:focus-visible {
    opacity: 1;
  }
  .att__delete:hover {
    color: var(--error);
  }
  .att__delete:disabled {
    opacity: 0.5;
  }

  .att__confirm {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.5rem;
    margin-top: 0.75rem;
    padding-left: 0.625rem;
    border-left: 2px solid var(--error);
  }
  .att__confirm-text {
    font-size: 0.75rem;
    color: var(--text-muted);
  }
  .att__confirm-go {
    font-size: 0.75rem;
    font-weight: 500;
    padding: 0.25rem 0.5rem;
    border-radius: 0.375rem;
    color: var(--error-text);
    background: var(--error);
  }
  .att__confirm-go:disabled {
    opacity: 0.5;
  }
  .att__confirm-cancel {
    font-size: 0.75rem;
    padding: 0.25rem 0.5rem;
    border-radius: 0.375rem;
    color: var(--text-muted);
  }
  .att__confirm-cancel:hover {
    background: var(--bg-subtle);
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
  .att__stack .att__item {
    width: 100%;
  }
  /* A file row has its own icon on the left; sit beside Delete instead. */
  .att__stack .att__insert {
    left: auto;
    right: 1.875rem;
  }
  .att__stack .att__info {
    left: auto;
    right: 3.5rem;
  }

  .att__detail {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    margin-top: 0.75rem;
    padding: 0.625rem 0.75rem;
    border: 1px solid var(--border);
    border-radius: 0.5rem;
    background: var(--bg-subtle);
    font-size: 0.75rem;
  }
  .att__detail-head {
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }
  .att__detail-name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-weight: 500;
    color: var(--text);
  }
  .att__detail-close {
    display: inline-flex;
    padding: 0.125rem;
    border-radius: 0.25rem;
    color: var(--text-muted);
  }
  .att__detail-close:hover {
    color: var(--text);
  }
  .att__detail-row {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 0.25rem 0.75rem;
  }
  .att__detail-label {
    width: 4.5rem;
    flex-shrink: 0;
    color: var(--text-faint);
  }
  .att__detail-value {
    color: var(--text-muted);
  }
  .att__detail-alt {
    display: flex;
    flex: 1;
    min-width: 12rem;
    gap: 0.375rem;
  }
  .att__detail-input {
    flex: 1;
    min-width: 0;
    padding: 0.25rem 0.375rem;
    border: 1px solid var(--border);
    border-radius: 0.3125rem;
    background: var(--surface);
    color: var(--text);
    outline: none;
  }
  .att__detail-input:focus {
    border-color: var(--accent);
  }
  .att__detail-save {
    padding: 0.25rem 0.5rem;
    border-radius: 0.375rem;
    font-weight: 500;
    background: var(--accent);
    color: var(--accent-text);
  }
  .att__detail-save:disabled {
    opacity: 0.5;
  }
  .att__detail-uses {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
    flex: 1;
    min-width: 12rem;
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .att__detail-uses li {
    display: flex;
    gap: 0.5rem;
    min-width: 0;
  }
  .att__detail-link {
    flex-shrink: 0;
    font-family: var(--font-mono);
    color: var(--accent);
  }
  .att__detail-title {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--text-muted);
  }
  .att__stack .att__item > :global(*:first-child) {
    flex: 1;
    min-width: 0;
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

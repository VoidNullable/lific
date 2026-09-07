<script lang="ts">
  // LIF-465: the publish/unpublish control on the project overview.
  //
  // Publishing is a disclosure, not a preference, so turning it on is two
  // steps (read the warning, tick the acknowledgement) and turning it off is
  // one: the safe direction is never gated. The warning is specific because
  // "anyone with the link" would be a lie; there is no secret in the address.
  import { updateProject, type Project } from "./api";
  import { copyToClipboard } from "./clipboard";
  import { toast } from "./toast/toast.svelte";
  import { Globe, Copy, ExternalLink, AlertTriangle } from "lucide-svelte";

  let {
    project,
    onChange,
  }: {
    project: Project;
    /** Called with the updated project so the parent's copy stays in step. */
    onChange: (project: Project) => void;
  } = $props();

  let acknowledged = $state(false);
  let saving = $state(false);
  let error = $state("");

  let url = $derived(`${window.location.origin}/public/${project.identifier}`);

  async function setPublished(next: boolean) {
    saving = true;
    error = "";
    // Only `is_public` is sent. A form that also posted the name or the lead
    // would make publishing carry edits the operator did not intend.
    const res = await updateProject(project.id, { is_public: next });
    saving = false;
    if (!res.ok) {
      error = res.error;
      return;
    }
    acknowledged = false;
    onChange(res.data);
    toast(next ? "Project published" : "Public access turned off", {
      kind: next ? "success" : "info",
    });
  }
</script>

<section class="rounded-xl border border-[var(--border)] bg-[var(--surface)] overflow-hidden">
  <div class="flex items-start gap-3 px-4 py-3.5 border-b border-[var(--border)]">
    <Globe size={15} class="mt-0.5 shrink-0 text-[var(--text-muted)]" />
    <div class="flex-1 min-w-0">
      <h2 class="text-body-sm font-semibold text-[var(--text)]">
        Public issue view
      </h2>
      <p class="text-caption text-[var(--text-muted)] mt-0.5">
        {project.is_public
          ? "This project's current issues are readable by anyone, with no account."
          : "Off. Only people with access to this project can see its issues."}
      </p>
    </div>
    <span
      class="shrink-0 text-caption px-2 py-0.5 rounded-full border"
      class:published={project.is_public}
      style={project.is_public
        ? "border-color: color-mix(in oklab, var(--success) 45%, transparent); color: var(--success)"
        : "border-color: var(--border); color: var(--text-faint)"}
    >
      {project.is_public ? "Public" : "Private"}
    </span>
  </div>

  <div class="px-4 py-4 flex flex-col gap-4">
    {#if project.is_public}
      <div class="flex flex-col gap-2">
        <p class="text-caption text-[var(--text-muted)]">Public address</p>
        <div class="flex items-center gap-2">
          <code
            class="flex-1 min-w-0 truncate text-body-sm font-mono px-2.5 py-1.5
                   rounded-md border border-[var(--border)] bg-[var(--bg-subtle)]"
          >{url}</code>
          <button
            type="button"
            class="shrink-0 size-8 grid place-items-center rounded-md border
                   border-[var(--border)] text-[var(--text-muted)]
                   hover:text-[var(--text)] hover:bg-[var(--bg-subtle)] transition-colors"
            aria-label="Copy public link"
            onclick={() => copyToClipboard(url, "public link")}
          >
            <Copy size={14} />
          </button>
          <a
            class="shrink-0 size-8 grid place-items-center rounded-md border
                   border-[var(--border)] text-[var(--text-muted)]
                   hover:text-[var(--text)] hover:bg-[var(--bg-subtle)] transition-colors"
            aria-label="Open public view"
            href={url}
            target="_blank"
            rel="noopener noreferrer"
          >
            <ExternalLink size={14} />
          </a>
        </div>
        <p class="text-caption text-[var(--text-faint)]">
          The address stays the same. Turning public access off and on again
          brings this exact link back.
        </p>
      </div>

      <div class="flex items-center gap-3 pt-1">
        <button
          type="button"
          class="text-body-sm font-medium px-3 py-1.5 rounded-md border
                 border-[var(--border)] hover:bg-[var(--bg-subtle)]
                 transition-colors disabled:opacity-50"
          disabled={saving}
          onclick={() => setPublished(false)}
        >
          {saving ? "Working…" : "Turn off public access"}
        </button>
        <p class="text-caption text-[var(--text-faint)]">
          Closes the link immediately. Copies people already downloaded stay
          downloaded.
        </p>
      </div>
    {:else}
      <div
        class="rounded-lg border p-3 flex gap-2.5"
        style="border-color: color-mix(in oklab, var(--warn) 35%, transparent);
               background: color-mix(in oklab, var(--warn) 8%, transparent)"
      >
        <AlertTriangle size={15} class="mt-0.5 shrink-0 text-[var(--warn)]" />
        <div class="text-body-sm text-[var(--text)] flex flex-col gap-2">
          <p class="font-medium">
            Publishing makes existing content public, not just future content.
          </p>
          <ul class="list-disc pl-4 flex flex-col gap-1 text-[var(--text-muted)]">
            <li>
              Every current issue in this project, including its full
              description.
            </li>
            <li>Every comment on those issues.</li>
            <li>
              Every file attached to those issues and comments, downloadable by
              anyone.
            </li>
          </ul>
          <p class="text-[var(--text-muted)]">
            Pages, plans, history, deleted items and your member list stay
            private. Nobody needs an account or a password, and there is no
            secret in the link, so search engines can find it. Turning this
            back off closes the link but cannot recall anything already
            downloaded.
          </p>
        </div>
      </div>

      <label class="flex items-start gap-2.5 text-body-sm cursor-pointer">
        <input
          type="checkbox"
          bind:checked={acknowledged}
          class="mt-0.5 accent-[var(--accent)]"
        />
        <span class="text-[var(--text-muted)]">
          I've reviewed this project's issues, comments and attachments, and
          they can be public.
        </span>
      </label>

      <div>
        <button
          type="button"
          class="text-body-sm font-medium px-3 py-1.5 rounded-md
                 bg-[var(--btn-success)] text-[var(--btn-success-text)]
                 hover:bg-[var(--btn-success-hover)] transition-colors
                 disabled:opacity-40 disabled:cursor-not-allowed"
          disabled={!acknowledged || saving}
          onclick={() => setPublished(true)}
        >
          {saving ? "Publishing…" : "Publish issues"}
        </button>
      </div>
    {/if}

    {#if error}
      <p class="text-body-sm text-[var(--error)]">{error}</p>
    {/if}
  </div>
</section>

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

  // LIF-583: there is no separate public view. A published project is read
  // at its normal links, so the link to share is the overview's.
  let url = $derived(`${window.location.origin}/${project.identifier}/overview`);

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
        Public access
      </h2>
      <p class="text-caption text-[var(--text-muted)] mt-0.5">
        {project.is_public
          ? "Anyone can read this project at its normal links, with no account. Only members can change anything."
          : "Off. Only people with access to this project can read it."}
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
        <p class="text-caption text-[var(--text-muted)]">Project link</p>
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
            aria-label="Copy project link"
            onclick={() => copyToClipboard(url, "project link")}
          >
            <Copy size={14} />
          </button>
          <a
            class="shrink-0 size-8 grid place-items-center rounded-md border
                   border-[var(--border)] text-[var(--text-muted)]
                   hover:text-[var(--text)] hover:bg-[var(--bg-subtle)] transition-colors"
            aria-label="Open project link"
            href={url}
            target="_blank"
            rel="noopener noreferrer"
          >
            <ExternalLink size={14} />
          </a>
        </div>
        <p class="text-caption text-[var(--text-faint)]">
          This is the project's ordinary link, and so is every issue, page
          and plan link inside it. Someone without an account can read them;
          a member who signs in can edit as usual.
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
          Signed-out readers lose access immediately. Copies people already
          downloaded stay downloaded.
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
              Every issue, page, plan and module in this project, with full
              descriptions, labels and folders.
            </li>
            <li>
              Every comment, the activity history and insights, with the
              names of the people and agents behind them.
            </li>
            <li>
              Every file attached to this project, downloadable by anyone.
            </li>
          </ul>
          <p class="text-[var(--text-muted)]">
            Your member list and project settings stay private, and only
            members can change anything. Nobody needs an account or a
            password to read, and there is no secret in the link, so search
            engines can find it. Turning this back off closes the project to
            readers but cannot recall anything already downloaded.
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
          I've reviewed this project's issues, pages, plans, comments and
          files, and they can be public.
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
          {saving ? "Publishing…" : "Publish project"}
        </button>
      </div>
    {/if}

    {#if error}
      <p class="text-body-sm text-[var(--error)]">{error}</p>
    {/if}
  </div>
</section>

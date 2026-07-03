<script lang="ts">
  // LIF-264: GitHub Issues → Lific import panel.
  //
  // A three-step flow mirroring the `connect` stepped modal: (1) configure the
  // repo + token + status mapping, (2) preview (dry-run) the counts, (3) run
  // the real import and show the summary. Preview and run hit the same endpoint
  // with dry_run true/false, so the numbers the user confirms are the numbers
  // they get.
  import { importGithub, type ImportSummary } from "./api";
  import { Download, ArrowRight, Check, Loader, AlertTriangle } from "lucide-svelte";

  let {
    projectId,
    onImported,
  }: { projectId: number; onImported?: () => void } = $props();

  type Step = "configure" | "preview" | "done";
  let step = $state<Step>("configure");

  let repo = $state("");
  let token = $state("");
  let stateFilter = $state<"open" | "closed" | "all">("all");
  let mapOpen = $state("backlog");
  let mapClosed = $state("done");

  let busy = $state(false);
  let error = $state("");
  let preview = $state<ImportSummary | null>(null);
  let result = $state<ImportSummary | null>(null);

  const STATUSES = ["backlog", "todo", "active", "done", "cancelled"];

  function repoValid(): boolean {
    return /^[^/\s]+\/[^/\s]+$/.test(repo.trim());
  }

  async function runPreview() {
    if (!repoValid() || busy) return;
    busy = true;
    error = "";
    const res = await importGithub(projectId, {
      repo: repo.trim(),
      token: token.trim() || undefined,
      state: stateFilter,
      map_open: mapOpen,
      map_closed: mapClosed,
      dry_run: true,
    });
    busy = false;
    if (res.ok) {
      preview = res.data;
      step = "preview";
    } else {
      error = res.error;
    }
  }

  async function runImport() {
    if (busy) return;
    busy = true;
    error = "";
    const res = await importGithub(projectId, {
      repo: repo.trim(),
      token: token.trim() || undefined,
      state: stateFilter,
      map_open: mapOpen,
      map_closed: mapClosed,
      dry_run: false,
    });
    busy = false;
    if (res.ok) {
      result = res.data;
      step = "done";
      if (res.data.issues_created > 0) onImported?.();
    } else {
      error = res.error;
    }
  }

  function reset() {
    step = "configure";
    preview = null;
    result = null;
    error = "";
  }
</script>

<section>
  <div class="flex items-center gap-2 mb-1">
    <Download size={16} class="text-[var(--text-muted)]" />
    <h2 class="text-body-sm font-semibold text-[var(--text)]">Import from GitHub</h2>
  </div>
  <p class="text-caption text-[var(--text-muted)] mb-3 leading-relaxed">
    Pull issues from a GitHub repo into this project. Pull requests are skipped.
    Re-running never duplicates — already-imported issues are recognized and left alone.
  </p>

  <div class="rounded-xl bg-[var(--surface)] shadow-[0_1px_2px_rgba(0,0,0,0.06)] p-4">
    {#if step === "configure"}
      <div class="flex flex-col gap-4">
        <div>
          <label for="import-repo" class="block text-body-sm font-medium text-[var(--text)] mb-1.5">
            Repository
          </label>
          <input
            id="import-repo"
            bind:value={repo}
            placeholder="owner/name"
            spellcheck="false"
            class="w-full rounded-md px-3 py-2 text-body font-mono
                   border border-[var(--border)] bg-[var(--bg-subtle)] text-[var(--text)]
                   placeholder:text-[var(--text-faint)] outline-none transition-colors
                   focus:border-[var(--accent)] focus:shadow-[0_0_0_3px_var(--accent-subtle)]"
          />
        </div>

        <div>
          <label for="import-token" class="block text-body-sm font-medium text-[var(--text)] mb-1.5">
            Token <span class="text-caption text-[var(--text-faint)]">optional for public repos</span>
          </label>
          <input
            id="import-token"
            bind:value={token}
            type="password"
            placeholder="ghp_…"
            spellcheck="false"
            class="w-full rounded-md px-3 py-2 text-body font-mono
                   border border-[var(--border)] bg-[var(--bg-subtle)] text-[var(--text)]
                   placeholder:text-[var(--text-faint)] outline-none transition-colors
                   focus:border-[var(--accent)] focus:shadow-[0_0_0_3px_var(--accent-subtle)]"
          />
        </div>

        <div class="flex gap-4 flex-wrap">
          <div>
            <label for="import-state" class="block text-body-sm font-medium text-[var(--text)] mb-1.5">Issues</label>
            <select
              id="import-state"
              bind:value={stateFilter}
              class="text-body-sm rounded-md border border-[var(--border)] bg-[var(--bg-subtle)]
                     text-[var(--text)] px-2.5 py-2 outline-none focus:border-[var(--accent)]"
            >
              <option value="all">Open + closed</option>
              <option value="open">Open only</option>
              <option value="closed">Closed only</option>
            </select>
          </div>
          <div>
            <label for="map-open" class="block text-body-sm font-medium text-[var(--text)] mb-1.5">Open →</label>
            <select
              id="map-open"
              bind:value={mapOpen}
              class="text-body-sm rounded-md border border-[var(--border)] bg-[var(--bg-subtle)]
                     text-[var(--text)] px-2.5 py-2 outline-none focus:border-[var(--accent)]"
            >
              {#each STATUSES as s}<option value={s}>{s}</option>{/each}
            </select>
          </div>
          <div>
            <label for="map-closed" class="block text-body-sm font-medium text-[var(--text)] mb-1.5">Closed →</label>
            <select
              id="map-closed"
              bind:value={mapClosed}
              class="text-body-sm rounded-md border border-[var(--border)] bg-[var(--bg-subtle)]
                     text-[var(--text)] px-2.5 py-2 outline-none focus:border-[var(--accent)]"
            >
              {#each STATUSES as s}<option value={s}>{s}</option>{/each}
            </select>
          </div>
        </div>

        {#if error}
          <div class="flex items-start gap-2 text-caption text-[var(--error)]">
            <AlertTriangle size={13} class="mt-0.5 shrink-0" />
            <span>{error}</span>
          </div>
        {/if}

        <div>
          <button
            class="inline-flex items-center gap-1.5 text-body-sm font-medium
                   text-[var(--btn-success-text)] bg-[var(--btn-success)] px-3.5 py-2 rounded-md
                   hover:bg-[var(--btn-success-hover)] transition-colors
                   disabled:opacity-40 disabled:cursor-not-allowed"
            disabled={!repoValid() || busy}
            onclick={runPreview}
          >
            {#if busy}<Loader size={14} class="animate-spin" /> Previewing…{:else}Preview import <ArrowRight size={14} />{/if}
          </button>
        </div>
      </div>

    {:else if step === "preview" && preview}
      <div class="flex flex-col gap-4">
        <p class="text-body-sm text-[var(--text-muted)]">
          Previewing <span class="font-mono text-[var(--text)]">{repo.trim()}</span>. This will create:
        </p>
        <div class="grid grid-cols-3 gap-3">
          {@render stat("Issues", preview.issues_created)}
          {@render stat("Comments", preview.comments_planned)}
          {@render stat("Labels", preview.labels_planned)}
        </div>
        {#if preview.issues_skipped_existing > 0 || preview.skipped_non_issues > 0}
          <p class="text-caption text-[var(--text-faint)]">
            {#if preview.issues_skipped_existing > 0}
              {preview.issues_skipped_existing} already imported (will be skipped).
            {/if}
            {#if preview.skipped_non_issues > 0}
              {preview.skipped_non_issues} pull request(s) excluded.
            {/if}
          </p>
        {/if}

        {#if error}
          <div class="flex items-start gap-2 text-caption text-[var(--error)]">
            <AlertTriangle size={13} class="mt-0.5 shrink-0" /><span>{error}</span>
          </div>
        {/if}

        <div class="flex items-center gap-2">
          <button
            class="inline-flex items-center gap-1.5 text-body-sm font-medium
                   text-[var(--btn-success-text)] bg-[var(--btn-success)] px-3.5 py-2 rounded-md
                   hover:bg-[var(--btn-success-hover)] transition-colors
                   disabled:opacity-40 disabled:cursor-not-allowed"
            disabled={busy || preview.issues_created === 0}
            onclick={runImport}
          >
            {#if busy}<Loader size={14} class="animate-spin" /> Importing…{:else}Import {preview.issues_created} issue{preview.issues_created !== 1 ? "s" : ""}{/if}
          </button>
          <button
            class="text-body-sm text-[var(--text-muted)] px-3 py-2 rounded-md hover:bg-[var(--bg-subtle)] transition-colors"
            disabled={busy}
            onclick={reset}
          >
            Back
          </button>
        </div>
      </div>

    {:else if step === "done" && result}
      <div class="flex flex-col gap-4">
        <div class="flex items-center gap-2 text-[var(--success)]">
          <span class="grid place-items-center size-7 rounded-full bg-[var(--success-bg)]"><Check size={15} /></span>
          <span class="text-body font-medium text-[var(--text)]">Import complete</span>
        </div>
        <div class="grid grid-cols-3 gap-3">
          {@render stat("Issues", result.issues_created)}
          {@render stat("Comments", result.comments_created)}
          {@render stat("Labels", result.labels_created)}
        </div>
        {#if result.issues_skipped_existing > 0}
          <p class="text-caption text-[var(--text-faint)]">
            {result.issues_skipped_existing} already-imported issue(s) skipped.
          </p>
        {/if}
        <div>
          <button
            class="text-body-sm text-[var(--text-muted)] border border-[var(--border)] px-3 py-1.5 rounded-md hover:bg-[var(--bg-subtle)] transition-colors"
            onclick={reset}
          >
            Import another repo
          </button>
        </div>
      </div>
    {/if}
  </div>
</section>

{#snippet stat(label: string, value: number)}
  <div class="rounded-lg bg-[var(--bg-subtle)] px-3 py-2.5 text-center">
    <div class="text-display-sm font-display text-[var(--text)] tabular-nums">{value}</div>
    <div class="text-caption text-[var(--text-muted)]">{label}</div>
  </div>
{/snippet}

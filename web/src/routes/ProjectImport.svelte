<script lang="ts">
  import { getContext, onMount } from "svelte";
  import { ArrowLeft } from "lucide-svelte";
  import { getArchiveCapabilities, me, onSessionChange, type ArchiveCapabilities } from "../lib/api";
  import { archiveImport, restoreArchiveImport, startArchiveImport, resetArchiveImport } from "../lib/archiveImport.svelte";

  let { navigate }: { navigate: (path: string) => void } = $props();
  const topbarCtx = getContext<{ set: (s: import("svelte").Snippet | undefined) => void } | undefined>("lific:topbar");
  $effect(() => { topbarCtx?.set(topbarContent); return () => topbarCtx?.set(undefined); });
  let capabilities = $state<ArchiveCapabilities | null>(null);
  let loading = $state(true);
  let accessError = $state("");
  let file = $state<File | null>(null);
  let validation = $state("");
  let confirmed = $state(false);
  const busy = $derived(archiveImport.phase === "uploading" || archiveImport.phase === "processing");
  const result = $derived(archiveImport.result);
  const totalRows = $derived(result ? Object.values(result.report.rows).reduce((sum, n) => sum + n, 0) : 0);
  const mib = (bytes: number) => `${(bytes / 1024 / 1024).toLocaleString()} MiB`;

  onMount(() => {
    let generation = 0;
    async function initialize() {
      const current = ++generation;
      const token = localStorage.getItem("lific_token");
      loading = true; capabilities = null; accessError = "";
      file = null; confirmed = false; validation = "";
      if (!token) { accessError = "Sign in to import a project archive."; loading = false; return; }
      const [res, user] = await Promise.all([getArchiveCapabilities(), me()]);
      if (current !== generation || token !== localStorage.getItem("lific_token")) return;
      if (res.ok && user.ok && token) {
        restoreArchiveImport(user.data.id, token);
        capabilities = res.data;
      } else accessError = !res.ok ? res.error : !user.ok ? user.error : "Sign in to import a project archive.";
      loading = false;
    }
    void initialize();
    const unsubscribe = onSessionChange(() => { void initialize(); });
    return () => { generation++; unsubscribe(); };
  });

  function selectFile(event: Event) {
    if (archiveImport.phase === "error") resetArchiveImport();
    file = (event.currentTarget as HTMLInputElement).files?.[0] ?? null;
    confirmed = false; validation = "";
    if (!file || !capabilities) return;
    if (!file.name.toLowerCase().endsWith(".tar.gz")) validation = "Choose a Lific project archive ending in .tar.gz.";
    else if (!file.size) validation = "This file is empty. Choose a Lific project archive.";
    else if (file.size > capabilities.max_upload_bytes) validation = `This archive exceeds the ${mib(capabilities.max_upload_bytes)} web upload limit. Use the CLI for larger archives.`;
  }

  function submit(event: SubmitEvent) {
    event.preventDefault();
    if (!capabilities?.can_import || !file || validation || !confirmed || busy) return;
    void startArchiveImport(file);
  }
</script>

<div class="h-full overflow-y-auto">
  <div class="mx-auto w-full max-w-[680px] px-6 py-8 space-y-5 break-words">
    <h1 class="text-xl font-semibold text-[var(--text)]">Import project archive</h1>
    {#if loading}
      <p role="status" class="text-body-sm text-[var(--text-muted)]">Checking archive access...</p>
    {:else if !capabilities?.can_import}
      <p role="alert" class="text-body-sm text-[var(--text-muted)]">{accessError || "Only a signed-in instance admin can import a project archive."}</p>
    {:else if archiveImport.phase === "success" && result}
      <div role="status" class="space-y-2">
        <h2 class="text-body-sm font-semibold text-[var(--text)]">{result.project.identifier} imported</h2>
        <p class="text-body-sm text-[var(--text-muted)]">The new project is private. You are its lead. The source project is unchanged.</p>
        <p class="text-body-sm text-[var(--text)]">{totalRows.toLocaleString()} {totalRows === 1 ? "record" : "records"} and {result.report.blobs.toLocaleString()} {result.report.blobs === 1 ? "file" : "files"} imported.</p>
      </div>
      <details class="text-body-sm text-[var(--text-muted)]">
        <summary class="cursor-pointer">Record counts</summary>
        <dl class="mt-2 space-y-1">
          {#each Object.entries(result.report.rows) as [table, count]}
            <div class="flex justify-between gap-4"><dt>{table.replaceAll("_", " ")}</dt><dd>{count.toLocaleString()}</dd></div>
          {/each}
        </dl>
      </details>
      {#if result.report.external_reference_count > 0}
        <section class="space-y-2" aria-labelledby="unresolved-heading">
          <h2 id="unresolved-heading" class="text-body-sm font-semibold text-[var(--text)]">{result.report.external_reference_count.toLocaleString()} unresolved {result.report.external_reference_count === 1 ? "reference" : "references"}</h2>
          <p class="text-body-sm text-[var(--text-muted)]">These references point outside the archive. Review them before relying on the imported links.</p>
          <ul class="list-disc pl-5 space-y-1 text-body-sm text-[var(--text-muted)] [overflow-wrap:anywhere]">
            {#each result.report.external_references as reference}<li>{reference}</li>{/each}
          </ul>
          {#if result.report.external_reference_count > result.report.external_references.length}
            <p class="text-caption text-[var(--text-muted)]">Showing the first {result.report.external_references.length.toLocaleString()}. The full report is retained with the project archive provenance.</p>
          {/if}
        </section>
      {:else}<p class="text-body-sm text-[var(--text-muted)]">No unresolved references.</p>{/if}
      <div class="flex flex-wrap gap-3">
        <button class="bg-[var(--accent)] text-[var(--accent-text)] rounded-md px-3 py-2 text-body-sm font-medium" onclick={() => navigate(`/${result.project.identifier}/overview`)}>Open imported project</button>
        <button class="toolbar-pill" onclick={() => { resetArchiveImport(); file = null; confirmed = false; }}>Import another archive</button>
      </div>
    {:else if archiveImport.phase === "unknown"}
      <p role="alert" class="text-body-sm text-[var(--error)]">{archiveImport.error}</p>
      <button class="toolbar-pill" onclick={() => navigate("/settings")}>Check project list</button>
      <div><button class="text-body-sm text-[var(--text-muted)] underline" onclick={resetArchiveImport}>I've checked the project list</button></div>
    {:else}
      <p class="text-body-sm text-[var(--text-muted)]">Create a private project from a Lific archive. You become its lead. Accounts and permissions do not transfer; imported author names are text only.</p>
      <p class="text-body-sm text-[var(--text-muted)]">The source stays untouched. The archive's project identifier must be unused here: import never merges or overwrites a project.</p>
      <form onsubmit={submit} class="space-y-4">
        <div class="space-y-2">
          <label for="project-archive" class="block text-body-sm font-medium text-[var(--text)]">Project archive (.tar.gz)</label>
          <input id="project-archive" type="file" accept=".tar.gz,application/gzip" onchange={selectFile} disabled={busy} aria-describedby="archive-limits" class="block w-full min-w-0 text-body-sm text-[var(--text-muted)] file:mr-3 file:rounded-md file:border file:border-[var(--border)] file:bg-[var(--bg-subtle)] file:px-3 file:py-2 file:text-[var(--text)]" />
          <p id="archive-limits" class="text-caption text-[var(--text-muted)]">Up to {mib(capabilities.max_upload_bytes)} compressed, {mib(capabilities.max_expanded_bytes)} expanded. Larger archives need the CLI.</p>
          {#if file}<p class="text-body-sm text-[var(--text)] [overflow-wrap:anywhere]">{file.name} ({mib(file.size)})</p>{/if}
          {#if validation}<p role="alert" class="text-body-sm text-[var(--error)]">{validation}</p>{/if}
        </div>
        <label class="flex items-start gap-2 text-body-sm text-[var(--text)]">
          <input type="checkbox" bind:checked={confirmed} disabled={busy || !file || !!validation} class="mt-1 accent-[var(--accent)]" />
          I understand this imports linked files, history and deleted content that may contain sensitive information.
        </label>
        {#if archiveImport.error}<p role="alert" class="text-body-sm text-[var(--error)]">{archiveImport.error}</p>{/if}
        {#if busy}
          <div role="status" class="space-y-2 text-body-sm text-[var(--text-muted)]">
            <p>{archiveImport.phase === "processing" ? "Importing..." : archiveImport.progress === null ? "Uploading..." : `Uploading... ${archiveImport.progress}%`}</p>
            {#if archiveImport.phase === "uploading"}<progress aria-label="Archive upload" max="100" value={archiveImport.progress ?? undefined} class="w-full accent-[var(--accent)]"></progress>{/if}
            <p>Once processing starts, it cannot be canceled. You can return to this page to see the result. If the connection is lost, check the project list before importing again.</p>
          </div>
        {/if}
        <button type="submit" disabled={!file || !!validation || !confirmed || busy} class="bg-[var(--accent)] text-[var(--accent-text)] rounded-md px-3 py-2 text-body-sm font-medium disabled:opacity-40 disabled:cursor-not-allowed">{busy ? "Import in progress" : "Import as private project"}</button>
      </form>
    {/if}
  </div>
</div>

{#snippet topbarContent()}
  <div class="flex flex-wrap items-center gap-3 px-6 py-2">
    <button class="toolbar-pill" onclick={() => navigate("/projects/new")}><ArrowLeft size={14} /> New project</button>
    <span class="text-body-sm text-[var(--text-muted)]">Import archive</span>
  </div>
{/snippet}

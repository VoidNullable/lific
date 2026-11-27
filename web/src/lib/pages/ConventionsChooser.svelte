<script lang="ts">
  // LIF-524: the Conventions tab before the page has any text. Someone who
  // can edit reads each template in full and starts from one; everyone else
  // is told there are no conventions yet.

  import {
    createPage,
    updatePage,
    listConventionTemplates,
    type ConventionTemplate,
    type ConventionTemplateKey,
  } from "../api";
  import type { PageRow } from "../sync/types";
  import { refreshProjectModel } from "../sync/readModel.svelte";
  import { toast } from "../toast/toast.svelte";
  import { getContext, type Snippet } from "svelte";
  import Breadcrumbs from "../Breadcrumbs.svelte";
  import SignInButton from "../SignInButton.svelte";
  import { isSignedOut } from "../signedOut";
  import ConventionsTemplatePicker from "./ConventionsTemplatePicker.svelte";
  import { CONVENTIONS_TITLE, defaultTemplateKey, templateName, type ConventionsState } from "./conventions";

  let {
    state: conv,
    projectId,
    projectIdentifier,
    canEdit,
    onStarted,
  }: {
    state: Exclude<ConventionsState<PageRow>, { kind: "page" }>;
    projectId: number;
    projectIdentifier: string;
    canEdit: boolean;
    /** The page to show now that it holds the chosen start. */
    onStarted: (pageId: number) => void;
  } = $props();

  const topbarCtx = getContext<{ set: (s: Snippet | undefined) => void } | undefined>("lific:topbar");
  $effect(() => {
    topbarCtx?.set(topbar);
    return () => topbarCtx?.set(undefined);
  });

  let templates = $state<ConventionTemplate[] | null>(null);
  let templatesError = $state("");
  let selected = $state<ConventionTemplateKey>("empty");
  let busy = $state(false);

  // Fetched once per mount, and only for someone who can use them.
  $effect(() => {
    if (canEdit && templates === null && !templatesError) void loadTemplates();
  });

  async function loadTemplates() {
    templatesError = "";
    const res = await listConventionTemplates();
    if (res.ok) {
      templates = res.data;
      selected = defaultTemplateKey(res.data);
    } else {
      templatesError = res.error;
    }
  }

  const chosen = $derived(templates?.find((t) => t.key === selected) ?? null);

  async function start() {
    if (busy || !canEdit || !chosen) return;
    busy = true;
    try {
      if (conv.kind === "empty") {
        const id = conv.page.id;
        if (chosen.content) {
          const res = await updatePage(id, { content: chosen.content });
          if (!res.ok) {
            toast(`Couldn't fill in ${conv.page.identifier}: ${res.error}`, { kind: "error" });
            return;
          }
          refreshProjectModel(projectId);
        }
        onStarted(id);
        return;
      }
      const res = await createPage({
        project_id: projectId,
        title: CONVENTIONS_TITLE,
        status: "active",
        conventions: true,
        ...(chosen.content ? { content: chosen.content } : {}),
      });
      if (!res.ok) {
        toast(
          res.status === 409
            ? "This project already has a conventions page. Showing it now."
            : `Couldn't create the conventions page: ${res.error}`,
          { kind: "error" },
        );
        refreshProjectModel(projectId);
        return;
      }
      refreshProjectModel(projectId);
      onStarted(res.data.id);
    } finally {
      busy = false;
    }
  }
</script>

{#snippet topbar()}
  <div class="flex items-center gap-2 px-3 sm:px-6 py-2 w-full min-w-0">
    <Breadcrumbs
      segments={[
        { label: projectIdentifier, href: `#/${projectIdentifier}/overview`, mono: true, hideBelowSm: true, copy: projectIdentifier },
        { label: "Conventions" },
      ]}
    />
    {#if isSignedOut()}
      <div class="ml-auto"><SignInButton /></div>
    {/if}
  </div>
{/snippet}

<div class="h-full overflow-y-auto">
  <div
    class="max-w-[1080px] mx-auto px-4 pb-10 pt-6 sm:px-10 sm:pt-8"
    data-testid="conventions-chooser"
    data-state={conv.kind}
  >
    <header class="mb-6 sm:mb-8 max-w-[68ch]">
      <h1 class="text-title font-medium text-[var(--text)]">Conventions</h1>
      <p class="text-body text-[var(--text-muted)] mt-1">
        {#if canEdit}
          Agents read this page at the start of every session; pick the
          template closest to how you work, then make it yours.
        {:else if conv.kind === "none"}
          This project has no conventions yet.
        {:else}
          No conventions written yet.
        {/if}
      </p>
    </header>

    {#if canEdit}
      {#if templatesError}
        <p class="text-body-sm text-[var(--error)]" role="alert">
          Couldn't load the templates: {templatesError}.
          <button class="text-[var(--accent)] hover:underline" onclick={() => void loadTemplates()}>Try again</button>
        </p>
      {:else}
        <ConventionsTemplatePicker {templates} bind:selected disabled={busy}>
          {#snippet action(t)}
            <button
              type="button"
              data-testid="conventions-use-template"
              class="text-body-sm font-medium text-[var(--accent-text)] bg-[var(--accent)]
                     px-3.5 py-1.5 rounded-md hover:opacity-90 transition-opacity
                     focus-visible:outline-none focus-visible:shadow-[0_0_0_3px_var(--accent-subtle)]
                     disabled:opacity-50 disabled:cursor-not-allowed"
              disabled={busy}
              onclick={() => void start()}
            >
              {#if busy}
                Starting…
              {:else if t.key === "empty"}
                Start blank
              {:else}
                Use {templateName(t)}
              {/if}
            </button>
          {/snippet}
        </ConventionsTemplatePicker>
      {/if}
    {/if}
  </div>
</div>

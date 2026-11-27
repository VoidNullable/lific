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
  import { ScrollText } from "lucide-svelte";
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
    class="max-w-[960px] mx-auto px-4 py-6 sm:px-10 sm:py-8 flex flex-col gap-5"
    data-testid="conventions-chooser"
    data-state={conv.kind}
  >
    <header class="flex items-start gap-3">
      <span class="shrink-0 size-10 grid place-items-center rounded-lg bg-[var(--accent-subtle)] text-[var(--accent)]">
        <ScrollText size={20} />
      </span>
      <div class="flex-1 min-w-0">
        <h1 class="text-title font-medium text-[var(--text)]">Conventions</h1>
        <p class="text-body text-[var(--text-muted)] leading-relaxed mt-1 max-w-[62ch]">
          How work in this tracker should go: which module gets what, when an
          issue is marked for a person, who closes what. Agents read this page
          at the start of each session.
        </p>
      </div>
    </header>

    {#if !canEdit}
      <p class="text-body text-[var(--text-muted)] rounded-lg border border-[var(--border)] bg-[var(--surface)] px-4 py-3">
        {conv.kind === "none"
          ? "This project has no conventions yet."
          : "No conventions written yet."}
      </p>
    {:else}
      <section class="flex flex-col gap-3" aria-labelledby="conventions-start-heading">
        <div>
          <h2 id="conventions-start-heading" class="text-body-lg font-medium text-[var(--text)]">Pick a starting point</h2>
          <p class="text-body-sm text-[var(--text-muted)] mt-0.5">
            Read each template before you choose. The text becomes the page,
            and you can edit every line of it afterwards.
          </p>
        </div>

        {#if templatesError}
          <p class="text-body-sm text-[var(--error)]" role="alert">
            Couldn't load the templates: {templatesError}.
            <button class="text-[var(--accent)] hover:underline" onclick={() => void loadTemplates()}>Try again</button>
          </p>
        {:else}
          <ConventionsTemplatePicker {templates} bind:selected disabled={busy} previewHeight="min(28rem, 55dvh)" />
          <!-- On a phone the preview runs long, so the action stays in reach. -->
          <div
            class="sticky bottom-0 -mx-4 px-4 py-3 bg-[var(--bg)] border-t border-[var(--border)]
                   pb-[max(0.75rem,env(safe-area-inset-bottom))]
                   sm:static sm:mx-0 sm:px-0 sm:py-0 sm:border-0 sm:bg-transparent
                   flex flex-wrap items-center gap-3"
          >
            <button
              type="button"
              data-testid="conventions-use-template"
              class="text-body-sm font-medium text-[var(--btn-success-text)] bg-[var(--btn-success)]
                     px-4 py-2 rounded-md hover:bg-[var(--btn-success-hover)] transition-colors
                     disabled:opacity-50 disabled:cursor-not-allowed"
              disabled={busy || chosen === null}
              onclick={() => void start()}
            >
              {#if busy}
                Starting…
              {:else if chosen?.key === "empty"}
                Start with a blank page
              {:else if chosen}
                Use {templateName(chosen)}
              {:else}
                Use this template
              {/if}
            </button>
            {#if conv.kind === "empty"}
              <span class="text-caption text-[var(--text-faint)]">Fills in {conv.page.identifier}, which has no text yet.</span>
            {/if}
          </div>
        {/if}
      </section>
    {/if}
  </div>
</div>

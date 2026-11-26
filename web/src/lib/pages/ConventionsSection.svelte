<script lang="ts">
  // LIF-524: the Conventions block at the top of the Pages list. It shows
  // the project's conventions page when it has text, and otherwise offers
  // the templates to start one, so every member sees that the page exists.

  import {
    createPage,
    updatePage,
    listConventionTemplates,
    type ConventionTemplate,
  } from "../api";
  import type { PageRow } from "../sync/types";
  import { refreshProjectModel } from "../sync/readModel.svelte";
  import { toast } from "../toast/toast.svelte";
  import { ChevronRight, ScrollText } from "lucide-svelte";
  import Skeleton from "../Skeleton.svelte";
  import { CONVENTIONS_TITLE, startingTemplates, type ConventionsState } from "./conventions";

  let {
    state: conv,
    projectId,
    projectIdentifier,
    canEdit,
    navigate,
  }: {
    state: ConventionsState<PageRow>;
    projectId: number;
    projectIdentifier: string;
    canEdit: boolean;
    navigate: (path: string) => void;
  } = $props();

  const BLANK = "__blank";

  let templates = $state<ConventionTemplate[] | null>(null);
  let templatesError = $state("");
  /** The choice being applied, so its button can say so. */
  let busy = $state<string | null>(null);

  const needsTemplates = $derived(canEdit && conv.kind !== "page");

  // Fetched once per mount, and only when there is something to offer.
  $effect(() => {
    if (needsTemplates && templates === null && !templatesError) void loadTemplates();
  });

  async function loadTemplates() {
    templatesError = "";
    const res = await listConventionTemplates();
    if (res.ok) templates = startingTemplates(res.data);
    else templatesError = res.error;
  }

  function retryTemplates() {
    templates = null;
    templatesError = "";
  }

  const pagePath = (id: number) => `/${projectIdentifier}/pages/${id}`;

  async function choose(key: string, content: string) {
    if (busy || !canEdit) return;
    busy = key;
    try {
      if (conv.kind === "empty") {
        const res = await updatePage(conv.page.id, { content });
        if (!res.ok) {
          toast(`Couldn't fill in ${conv.page.identifier}: ${res.error}`, { kind: "error" });
          return;
        }
        refreshProjectModel(projectId);
        navigate(pagePath(res.data.id));
        return;
      }
      const res = await createPage({
        project_id: projectId,
        title: CONVENTIONS_TITLE,
        status: "active",
        conventions: true,
        ...(content ? { content } : {}),
      });
      if (!res.ok) {
        toast(
          res.status === 409
            ? "This project already has a conventions page. Reloading the list."
            : `Couldn't create the conventions page: ${res.error}`,
          { kind: "error" },
        );
        refreshProjectModel(projectId);
        return;
      }
      refreshProjectModel(projectId);
      navigate(pagePath(res.data.id));
    } finally {
      busy = null;
    }
  }

  const offerBlank = $derived(conv.kind === "none");
  const optionCount = $derived((templates?.length ?? 3) + (offerBlank ? 1 : 0));
</script>

<section
  class="mb-6 rounded-xl border border-[var(--border)] bg-[var(--surface)]
         shadow-[0_1px_2px_rgba(0,0,0,0.04)]"
  aria-labelledby="conventions-heading"
  data-testid="conventions-section"
  data-state={conv.kind}
>
  {#if conv.kind === "page"}
    {@const page = conv.page}
    {@const enabled = page.conventions_enabled !== false}
    <button
      class="group w-full flex items-start gap-3 p-3 sm:p-4 text-left rounded-xl
             hover:bg-[var(--bg-subtle)] transition-colors"
      onclick={() => navigate(pagePath(page.id))}
    >
      <span
        class="shrink-0 size-9 grid place-items-center rounded-lg
               {enabled
          ? 'bg-[var(--accent-subtle)] text-[var(--accent)]'
          : 'bg-[var(--bg-subtle)] text-[var(--text-faint)]'}"
      >
        <ScrollText size={18} />
      </span>
      <span class="flex-1 min-w-0 flex flex-col gap-0.5">
        <!-- A renamed page still says what it is. -->
        {#if page.title.trim().toLowerCase() !== "conventions"}
          <span class="text-micro font-semibold uppercase tracking-widest text-[var(--text-faint)]">
            Conventions
          </span>
        {/if}
        <span class="flex items-center gap-2 min-w-0">
          <span id="conventions-heading" class="text-body-lg font-medium text-[var(--text)] truncate">
            {page.title}
          </span>
          {#if !enabled}
            <span
              class="shrink-0 text-micro font-medium px-1.5 py-0.5 rounded-full
                     border border-[var(--border)] text-[var(--text-muted)]"
              data-testid="conventions-off"
            >
              Off
            </span>
          {/if}
          <span class="ml-auto hidden sm:inline text-caption font-mono text-[var(--text-faint)] shrink-0">
            {page.identifier}
          </span>
        </span>
        <span class="text-caption text-[var(--text-muted)]">
          {enabled
            ? "Agents read this with their briefing at the start of each session."
            : "Agents' briefings leave it out."}
        </span>
      </span>
      <ChevronRight
        size={16}
        class="shrink-0 self-center text-[var(--text-faint)] group-hover:text-[var(--text-muted)] transition-colors"
      />
    </button>
  {:else}
    <div class="p-3 sm:p-4 flex flex-col gap-3">
      <div class="flex items-start gap-3">
        <span class="shrink-0 size-9 grid place-items-center rounded-lg bg-[var(--accent-subtle)] text-[var(--accent)]">
          <ScrollText size={18} />
        </span>
        <div class="flex-1 min-w-0">
          <h2 id="conventions-heading" class="text-body-lg font-medium text-[var(--text)]">
            Conventions
          </h2>
          <p class="text-body-sm text-[var(--text-muted)] leading-relaxed mt-0.5">
            How work in this tracker should go: which module gets what, when an
            issue is marked for a person, who closes what. Agents read it with
            their briefing at the start of each session.
          </p>
        </div>
      </div>

      {#if !canEdit}
        <p class="text-body-sm text-[var(--text-faint)] sm:pl-12">
          {conv.kind === "none"
            ? "This project has no conventions yet."
            : "No conventions written yet."}
        </p>
      {:else}
        <div class="sm:pl-12 flex flex-col gap-2">
          <p class="text-micro font-semibold uppercase tracking-widest text-[var(--text-faint)]">
            Start from a template
          </p>

          {#if templatesError}
            <p class="text-body-sm text-[var(--text-muted)]" role="alert">
              Couldn't load the templates.
              <button class="text-[var(--accent)] hover:underline" onclick={retryTemplates}>Try again</button>
            </p>
          {/if}

          <div
            class="grid grid-cols-1 sm:grid-cols-2 gap-2
                   {optionCount === 3 ? 'xl:grid-cols-3' : 'xl:grid-cols-4'}"
            role="list"
          >
            {#if templates === null && !templatesError}
              {#each [0, 1, 2] as i (i)}
                <div class="rounded-lg border border-[var(--border)] p-3 flex flex-col gap-1.5" role="listitem">
                  <Skeleton variant="bar" class="h-3.5 w-28" />
                  <Skeleton variant="bar" class="h-3 w-full" />
                  <Skeleton variant="bar" class="h-3 w-2/3" />
                </div>
              {/each}
            {:else if templates}
              {#each templates as t (t.key)}
                <div role="listitem" class="flex">
                  <button
                    class="flex-1 flex flex-col gap-1 rounded-lg border border-[var(--border)] bg-[var(--bg)]
                           p-3 text-left transition-colors
                           hover:border-[var(--accent)] hover:bg-[var(--accent-subtle)]
                           focus-visible:outline-none focus-visible:border-[var(--accent)]
                           focus-visible:shadow-[0_0_0_3px_var(--accent-subtle)]
                           disabled:opacity-50 disabled:cursor-wait"
                    disabled={busy !== null}
                    data-template={t.key}
                    onclick={() => choose(t.key, t.content)}
                  >
                    <span class="text-body-sm font-medium text-[var(--text)]">
                      {busy === t.key ? "Creating…" : t.name}
                    </span>
                    {#if t.summary}
                      <span class="text-caption text-[var(--text-muted)] leading-snug">{t.summary}</span>
                    {/if}
                  </button>
                </div>
              {/each}
            {/if}

            {#if offerBlank}
              <div role="listitem" class="flex">
                <button
                  class="flex-1 flex flex-col gap-1 rounded-lg border border-dashed border-[var(--border)]
                         p-3 text-left transition-colors
                         hover:border-[var(--accent)] hover:bg-[var(--accent-subtle)]
                         focus-visible:outline-none focus-visible:border-[var(--accent)]
                         focus-visible:shadow-[0_0_0_3px_var(--accent-subtle)]
                         disabled:opacity-50 disabled:cursor-wait"
                  disabled={busy !== null}
                  data-template="blank"
                  onclick={() => choose(BLANK, "")}
                >
                  <span class="text-body-sm font-medium text-[var(--text)]">
                    {busy === BLANK ? "Creating…" : "Blank"}
                  </span>
                  <span class="text-caption text-[var(--text-muted)] leading-snug">Write your own from scratch.</span>
                </button>
              </div>
            {/if}
          </div>

          {#if conv.kind === "empty"}
            {@const existing = conv.page}
            <p class="text-caption text-[var(--text-faint)]">
              Or
              <button
                class="text-[var(--accent)] hover:underline"
                onclick={() => navigate(pagePath(existing.id))}
              >open {existing.identifier}</button>
              and write your own.
            </p>
          {/if}
        </div>
      {/if}
    </div>
  {/if}
</section>

<script lang="ts">
  // LIF-583: what `/` shows a signed-out reader when this instance has more
  // than one published project. With exactly one, App opens it instead; with
  // none, `/` is the login screen as it always was.
  import type { Project } from "../lib/api";
  import ProjectIcon from "../lib/ProjectIcon.svelte";
  import SignInButton from "../lib/SignInButton.svelte";
  import { navLink } from "../lib/navLink";
  import { getContext } from "svelte";

  let {
    navigate,
    projects,
  }: {
    navigate: (path: string) => void;
    projects: Project[];
  } = $props();

  const topbarCtx = getContext<{
    set: (s: import("svelte").Snippet | undefined) => void;
  } | undefined>("lific:topbar");

  $effect(() => {
    topbarCtx?.set(topbarContent);
    return () => topbarCtx?.set(undefined);
  });
</script>

{#snippet topbarContent()}
  <div class="flex items-center gap-3 px-3 md:px-6 h-12 md:h-[3.25rem] w-full">
    <h1 class="font-display text-heading font-medium tracking-tight text-[var(--text)] leading-none">
      Published projects
    </h1>
    <div class="flex-1"></div>
    <SignInButton />
  </div>
{/snippet}

<div class="max-w-3xl mx-auto px-4 md:px-8 py-8">
  <p class="text-body text-[var(--text-muted)] mb-6">
    These projects are open to read without an account. Sign in to edit or comment.
  </p>
  <ul class="flex flex-col gap-2" data-published-projects>
    {#each projects as project (project.id)}
      <li>
        <a
          href="#/{project.identifier}/overview"
          use:navLink={navigate}
          class="flex items-start gap-3 p-4 rounded-xl border border-[var(--border)] bg-[var(--surface)]
                 hover:border-[var(--text-faint)] transition-colors"
        >
          <span class="size-10 grid place-items-center shrink-0 rounded-lg bg-[var(--bg-subtle)]">
            {#if project.emoji}
              <ProjectIcon value={project.emoji} size={20} />
            {:else}
              <span class="text-caption font-mono font-semibold text-[var(--text-muted)]">
                {project.identifier.slice(0, 2)}
              </span>
            {/if}
          </span>
          <span class="min-w-0 flex-1">
            <span class="flex items-baseline gap-2">
              <span class="truncate text-body-lg font-medium text-[var(--text)]">{project.name}</span>
              <span class="shrink-0 font-mono text-micro text-[var(--text-faint)]">{project.identifier}</span>
            </span>
            {#if project.description}
              <span class="block mt-1 text-body-sm text-[var(--text-muted)] line-clamp-2">
                {project.description}
              </span>
            {/if}
          </span>
        </a>
      </li>
    {/each}
  </ul>
</div>

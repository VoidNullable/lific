<script lang="ts">
  // Shared thread renderer for issue and page comments. The parent owns the
  // comments array (so it can refetch / re-render); this component just renders
  // it and manages the local draft state for new comments.
  //
  // Extracted from IssueDetail.svelte for LIF-106 so PageDetail can reuse it.
  import Markdown from "./Markdown.svelte";
  import type { Comment } from "./api";

  let {
    comments,
    editable = true,
    onSubmit,
    placeholder = "Leave a comment... (markdown supported)",
  }: {
    comments: Comment[];
    editable?: boolean;
    /** Submit a new comment. Should resolve to the created Comment so the
     *  parent can append it, or null on failure. */
    onSubmit: (content: string) => Promise<Comment | null>;
    placeholder?: string;
  } = $props();

  let draft = $state("");
  let submitting = $state(false);

  async function submit() {
    const trimmed = draft.trim();
    if (!trimmed || submitting) return;
    submitting = true;
    const created = await onSubmit(trimmed);
    if (created) draft = "";
    submitting = false;
  }

  function formatDate(iso: string): string {
    const d = new Date(iso + "Z");
    return d.toLocaleDateString("en-US", {
      month: "short",
      day: "numeric",
      year: "numeric",
      hour: "numeric",
      minute: "2-digit",
    });
  }

  function formatRelative(iso: string): string {
    const d = new Date(iso + "Z");
    const now = new Date();
    const diffMs = now.getTime() - d.getTime();
    const diffMins = Math.floor(diffMs / 60000);
    const diffHrs = Math.floor(diffMs / 3600000);
    const diffDays = Math.floor(diffMs / 86400000);
    if (diffMins < 1) return "just now";
    if (diffMins < 60) return `${diffMins}m ago`;
    if (diffHrs < 24) return `${diffHrs}h ago`;
    if (diffDays < 7) return `${diffDays}d ago`;
    return d.toLocaleDateString("en-US", { month: "short", day: "numeric" });
  }

  function initials(name: string): string {
    return name
      .split(/[\s_-]+/)
      .slice(0, 2)
      .map((w) => w[0]?.toUpperCase() ?? "")
      .join("");
  }
</script>

<section>
  <h2
    class="text-[0.8125rem] font-semibold uppercase tracking-widest
           text-[var(--text-faint)] mb-4"
  >
    Comments
    {#if comments.length > 0}
      <span class="font-normal ml-1">{comments.length}</span>
    {/if}
  </h2>

  {#if comments.length === 0}
    <p class="text-[0.875rem] text-[var(--text-faint)] mb-6">
      No comments yet.
    </p>
  {:else}
    <div class="space-y-0 mb-6">
      {#each comments as comment (comment.id)}
        <div
          class="flex gap-3 py-4
                 border-t border-[var(--border)] first:border-t-0"
        >
          <!-- Avatar -->
          <div
            class="size-7 rounded-full bg-[var(--accent-subtle)]
                   text-[var(--accent)] flex items-center justify-center
                   text-[0.625rem] font-semibold shrink-0 mt-0.5"
          >
            {initials(comment.author_display_name || comment.author)}
          </div>

          <div class="flex-1 min-w-0">
            <div class="flex items-baseline gap-2 mb-1">
              <span class="text-[0.8125rem] font-medium text-[var(--text)]">
                {comment.author_display_name || comment.author}
              </span>
              <span
                class="text-[0.75rem] text-[var(--text-faint)]"
                title={formatDate(comment.created_at)}
              >
                {formatRelative(comment.created_at)}
              </span>
            </div>
            <Markdown
              content={comment.content}
              class="text-sm"
            />
          </div>
        </div>
      {/each}
    </div>
  {/if}

  <!-- New comment -->
  {#if editable}
    <div class="border-t border-[var(--border)] pt-4">
      <textarea
        bind:value={draft}
        class="w-full min-h-[80px] text-[0.875rem] leading-relaxed
               bg-[var(--surface)] border border-[var(--border)]
               rounded-md p-3 text-[var(--text)]
               resize-y outline-none placeholder:text-[var(--text-faint)]
               focus:border-[var(--accent)]
               focus:shadow-[0_0_0_3px_var(--accent-subtle)]"
        {placeholder}
        onkeydown={(e) => {
          if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) submit();
        }}
      ></textarea>
      <div class="flex items-center justify-between mt-2">
        <span class="text-[0.75rem] text-[var(--text-faint)]">
          Ctrl+Enter to submit
        </span>
        <button
          class="text-[0.8125rem] font-medium text-[var(--accent-text)]
                 bg-[var(--accent)] px-3 py-1.5 rounded-md
                 hover:bg-[var(--accent-hover)] transition-colors
                 disabled:opacity-50 disabled:cursor-not-allowed"
          disabled={!draft.trim() || submitting}
          onclick={submit}
        >
          {submitting ? "Posting..." : "Comment"}
        </button>
      </div>
    </div>
  {/if}
</section>

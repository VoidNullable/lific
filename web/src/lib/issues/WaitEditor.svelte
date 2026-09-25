<script lang="ts">
  // LIF-485: the issue sidebar's "Waiting on" field. Lists every user and
  // date blocker with its current state, and (for editors) adds and clears
  // them. Issue-to-issue blockers stay in the "Blocked by" field below.
  import {
    addIssueWait,
    clearIssueWait,
    listMentionCandidates,
    type CreateWaitInput,
    type IssueWait,
    type MentionCandidate,
  } from "../api";
  import { Hourglass, CalendarClock, CalendarCheck, CircleAlert, Plus, X } from "lucide-svelte";
  import { toast } from "../toast/toast.svelte";
  import { now } from "../now.svelte";
  import { describeWait, localDay } from "./waits";

  let {
    issueId,
    projectId,
    waits,
    editable,
    onChange,
  }: {
    issueId: number;
    projectId: number;
    waits: IssueWait[];
    editable: boolean;
    /** The issue's waits after a successful add or clear. */
    onChange: (waits: IssueWait[]) => void;
  } = $props();

  const today = $derived(localDay(new Date(now())));

  let adding = $state(false);
  let kind = $state<"user" | "date">("user");
  let user = $state("");
  let from = $state("");
  let until = $state("");
  let note = $state("");
  let busy = $state(false);
  let candidates = $state<MentionCandidate[] | null>(null);

  const valid = $derived(
    kind === "user" ? user !== "" : from !== "" && (until === "" || until >= from),
  );

  const TONE = {
    holding: "bg-[color-mix(in_srgb,var(--warn)_10%,transparent)]",
    due: "bg-[var(--accent-subtle)]",
    overdue: "bg-[var(--error-bg)]",
  } as const;
  const HEADLINE_TONE = {
    holding: "text-[var(--text)]",
    due: "text-[var(--accent)]",
    overdue: "text-[var(--error)] font-medium",
  } as const;

  async function openForm() {
    adding = true;
    kind = "user";
    user = "";
    from = "";
    until = "";
    note = "";
    if (candidates === null) {
      const res = await listMentionCandidates(projectId);
      candidates = res.ok ? res.data : [];
      if (!res.ok) toast(`Couldn't load members: ${res.error}`, { kind: "error" });
    }
  }

  async function submit(e: SubmitEvent) {
    e.preventDefault();
    if (!valid || busy) return;
    busy = true;
    const trimmed = note.trim();
    const input: CreateWaitInput =
      kind === "user"
        ? { user, note: trimmed }
        : { from, until: until || undefined, note: trimmed };
    const res = await addIssueWait(issueId, input);
    busy = false;
    if (!res.ok) {
      toast(`Couldn't add blocker: ${res.error}`, { kind: "error" });
      return;
    }
    adding = false;
    onChange([...waits, res.data]);
  }

  async function clear(wait: IssueWait) {
    if (busy) return;
    busy = true;
    const res = await clearIssueWait(issueId, wait.id);
    busy = false;
    if (!res.ok) {
      toast(`Couldn't clear blocker: ${res.error}`, { kind: "error" });
      return;
    }
    onChange(waits.filter((w) => w.id !== wait.id));
  }
</script>

<div class="issue-meta-field" data-testid="issue-waits">
  <p class="issue-meta-field-label">Waiting on</p>
  {#if waits.length > 0}
    <ul class="flex flex-col gap-1.5 m-0 p-0 list-none">
      {#each waits as wait (wait.id)}
        {@const described = describeWait(wait, today)}
        <li
          class="wait-item flex items-start gap-2 rounded-md px-2 py-1.5 -mx-2 {TONE[described.state]}"
          data-wait-kind={wait.kind}
          data-wait-state={described.state}
        >
          <span class="mt-0.5 shrink-0 {HEADLINE_TONE[described.state]}">
            {#if described.state === "overdue"}
              <CircleAlert size={13} />
            {:else if wait.kind === "user"}
              <Hourglass size={13} />
            {:else if described.state === "due"}
              <CalendarCheck size={13} />
            {:else}
              <CalendarClock size={13} />
            {/if}
          </span>
          <div class="flex-1 min-w-0">
            <p class="m-0 text-body-sm leading-snug break-words {HEADLINE_TONE[described.state]}">
              {described.headline}
            </p>
            {#if wait.note}
              <p class="m-0 mt-0.5 text-caption text-[var(--text-muted)] leading-snug break-words">
                {wait.note}
              </p>
            {/if}
          </div>
          {#if editable}
            <button
              type="button"
              class="size-6 -my-0.5 shrink-0 grid place-items-center rounded
                     text-[var(--text-faint)] hover:text-[var(--error)]
                     hover:bg-[var(--bg-subtle)] transition-colors disabled:opacity-40"
              title="Clear this blocker"
              aria-label="Clear blocker: {described.headline}"
              disabled={busy}
              onclick={() => clear(wait)}
            >
              <X size={13} />
            </button>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}

  {#if editable}
    {#if adding}
      <form
        class="flex flex-col gap-2 rounded-md border border-[var(--border)] p-2.5 -mx-1"
        onsubmit={submit}
        aria-label="Add blocker"
      >
        <div class="flex gap-1 p-0.5 rounded-md bg-[var(--bg-subtle)]" role="group" aria-label="Blocker kind">
          {#each [{ value: "user", label: "Person" }, { value: "date", label: "Dates" }] as option (option.value)}
            <button
              type="button"
              class="flex-1 text-caption font-medium px-2 py-1 rounded transition-colors
                     {kind === option.value
                ? 'bg-[var(--surface)] text-[var(--text)] shadow-sm'
                : 'text-[var(--text-muted)] hover:text-[var(--text)]'}"
              aria-pressed={kind === option.value}
              onclick={() => (kind = option.value as "user" | "date")}
            >
              {option.label}
            </button>
          {/each}
        </div>

        {#if kind === "user"}
          <label class="flex flex-col gap-1 text-caption text-[var(--text-muted)]">
            Person
            <select
              class="text-body-sm rounded-md border border-[var(--border)] bg-[var(--bg-subtle)]
                     text-[var(--text)] px-2 py-1.5 outline-none focus:border-[var(--accent)]"
              bind:value={user}
              disabled={candidates === null}
            >
              <option value="">{candidates === null ? "Loading members..." : "Choose a member"}</option>
              {#each candidates ?? [] as c (c.user_id)}
                <option value={c.username}>
                  {c.display_name && c.display_name !== c.username ? `${c.display_name} (@${c.username})` : `@${c.username}`}
                </option>
              {/each}
            </select>
          </label>
        {:else}
          <div class="flex gap-2">
            <label class="flex-1 min-w-0 flex flex-col gap-1 text-caption text-[var(--text-muted)]">
              From
              <input
                type="date"
                class="text-body-sm rounded-md border border-[var(--border)] bg-[var(--bg-subtle)]
                       text-[var(--text)] px-2 py-1.5 outline-none focus:border-[var(--accent)] min-w-0"
                bind:value={from}
                required
              />
            </label>
            <label class="flex-1 min-w-0 flex flex-col gap-1 text-caption text-[var(--text-muted)]">
              Until (optional)
              <input
                type="date"
                class="text-body-sm rounded-md border border-[var(--border)] bg-[var(--bg-subtle)]
                       text-[var(--text)] px-2 py-1.5 outline-none focus:border-[var(--accent)] min-w-0"
                bind:value={until}
                min={from || undefined}
              />
            </label>
          </div>
          <p class="m-0 text-caption text-[var(--text-faint)] leading-snug">
            Blocks until From. After Until it shows as overdue.
          </p>
        {/if}

        <label class="flex flex-col gap-1 text-caption text-[var(--text-muted)]">
          Note
          <input
            type="text"
            maxlength="500"
            class="text-body-sm rounded-md border border-[var(--border)] bg-[var(--bg-subtle)]
                   text-[var(--text)] px-2 py-1.5 outline-none focus:border-[var(--accent)]"
            placeholder={kind === "user" ? "What you need from them" : "Who said so, e.g. filing office"}
            bind:value={note}
          />
        </label>

        <div class="flex gap-2 justify-end">
          <button
            type="button"
            class="text-body-sm text-[var(--text-muted)] px-3 py-1.5 rounded-md hover:bg-[var(--bg-subtle)] transition-colors"
            onclick={() => (adding = false)}
          >
            Cancel
          </button>
          <button
            type="submit"
            class="text-body-sm font-medium text-[var(--btn-success-text)] bg-[var(--btn-success)]
                   px-3 py-1.5 rounded-md hover:bg-[var(--btn-success-hover)] transition-colors
                   disabled:opacity-40 disabled:cursor-not-allowed"
            disabled={!valid || busy}
          >
            Add
          </button>
        </div>
      </form>
    {:else}
      <button
        type="button"
        class="self-start flex items-center gap-1.5 text-body-sm text-[var(--text-muted)]
               rounded-md px-2 py-1 -mx-2 hover:bg-[var(--bg-subtle)] hover:text-[var(--text)] transition-colors"
        onclick={openForm}
      >
        <Plus size={13} />
        Add blocker
      </button>
    {/if}
  {/if}
</div>

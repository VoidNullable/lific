<script lang="ts">
  import {
    listUsers,
    listProjects,
    listProjectGroups,
    listConventionTemplates,
    type UserSummary,
    type ProjectGroup,
    type ConventionTemplate,
    type ConventionTemplateKey,
  } from "./api";
  import IconPicker from "./IconPicker.svelte";
  import Select from "./Select.svelte";
  import { folderPaths } from "./sidebarTree";
  import { FALLBACK_TEMPLATES } from "./pages/conventions";
  import { cleanIdentifier, identifierProblem, suggestIdentifier, IDENTIFIER_MAX } from "./projectIdentifier";
  import { ImagePlus, RotateCcw } from "lucide-svelte";
  import ConventionsTemplatePicker from "./pages/ConventionsTemplatePicker.svelte";

  // The create form. Its top half is the project's overview header, filled in
  // where it will appear; the rest is the setup a header doesn't show.
  let {
    name = $bindable(""),
    identifier = $bindable(""),
    description = $bindable(""),
    emoji = $bindable(""),
    leadUserId = $bindable<number | null>(null),
    groupId = $bindable<number | null>(null),
    conventions = $bindable<ConventionTemplateKey>("empty"),
    identifierError = $bindable<string | null>(null),
  }: {
    name?: string;
    identifier?: string;
    description?: string;
    emoji?: string;
    leadUserId?: number | null;
    /** Sidebar group to file the project into. The form only collects it;
     *  persisting it is the caller's job, since it rides a separate endpoint. */
    groupId?: number | null;
    /** LIF-524: the conventions page's starting template. */
    conventions?: ConventionTemplateKey;
    /** Why the identifier would be refused, for the caller's Create button. */
    identifierError?: string | null;
  } = $props();

  let users = $state<UserSummary[]>([]);
  let groups = $state<ProjectGroup[]>([]);
  let takenBy = $state(new Map<string, string>());
  let identifierTouched = $state(false);

  $effect(() => {
    listUsers().then((res) => {
      if (res.ok) users = res.data;
    });
    listProjectGroups().then((res) => {
      if (res.ok) groups = res.data;
    });
    listProjects().then((res) => {
      if (res.ok) takenBy = new Map(res.data.map((p) => [p.identifier.toUpperCase(), p.name]));
    });
  });

  // LIF-524: names and summaries come from the server; without it the four
  // choices still show, by name only.
  // Until the answer is in, placeholders hold the space rather than the
  // nameless fallbacks flashing up and then gaining their summaries.
  let templates = $state<ConventionTemplate[] | null>(null);
  $effect(() => {
    listConventionTemplates().then((res) => {
      templates = res.ok && res.data.length > 0 ? res.data : FALLBACK_TEMPLATES;
    });
  });

  let suggested = $derived(suggestIdentifier(name, new Set(takenBy.keys())));

  $effect(() => {
    if (!identifierTouched) identifier = suggested;
  });

  $effect(() => {
    identifierError = identifierProblem(identifier, takenBy);
  });

  const PLACEHOLDERS = [
    "Half-Life 3",
    "Star Citizen 2",
    "Portal 3",
    "Aperture Science",
    "Rewriting Rust in Rust",
    "Is It DNS?",
    "TODO: Name This Later",
    "Untitled Goose Project",
    "Regex for Dummys",
    "Shovelware Simulator",
    "Moon Base Alpha",
    "Sentient Spreadsheet",
    "Banana for Scale",
  ];
  const placeholder = PLACEHOLDERS[Math.floor(Math.random() * PLACEHOLDERS.length)];

  let shownId = $derived(identifier || "PRO");

  let groupOptions = $derived([
    { value: null, label: "No folder" },
    ...folderPaths(groups).map((g) => ({ value: g.id, label: g.label })),
  ]);

  // Deactivated accounts stay in the user list for admins; they can't lead.
  let userOptions = $derived([
    { value: null, label: "No lead" },
    ...users
      .filter((u) => u.is_active !== false)
      .map((u) => ({
        value: u.id,
        label: u.display_name || u.username,
        username: u.username,
        is_admin: u.is_admin,
        created_at: u.created_at,
      })),
  ]);

  function formatMemberSince(iso: string): string {
    const d = new Date(iso + "Z");
    return d.toLocaleDateString("en-US", { month: "short", year: "numeric" });
  }

  function userInitials(name: string): string {
    return name.split(/[\s_-]+/).slice(0, 2).map((w) => w[0]?.toUpperCase() ?? "").join("");
  }

  function onIdentifierInput(e: Event) {
    const el = e.currentTarget as HTMLInputElement;
    const cleaned = cleanIdentifier(el.value);
    if (el.value !== cleaned) el.value = cleaned;
    identifier = cleaned;
    identifierTouched = true;
  }

  function useSuggested() {
    identifierTouched = false;
    identifier = suggested;
  }

  const fieldClass =
    "text-body-sm rounded-md border border-[var(--border)] bg-[var(--bg)] text-[var(--text)] " +
    "px-2.5 py-1.5 outline-none transition-colors placeholder:text-[var(--text-faint)] " +
    "focus:border-[var(--accent)] focus-visible:ring-2 focus-visible:ring-[var(--accent-subtle)]";
</script>

{#snippet row(label: string, help: string, control: import("svelte").Snippet, forId?: string)}
  <div class="flex flex-col sm:flex-row sm:items-start gap-2 sm:gap-8 px-4 py-4 border-t border-[var(--border)] first:border-t-0">
    <div class="sm:w-44 shrink-0">
      {#if forId}
        <label for={forId} class="block text-body-sm font-medium text-[var(--text)]">{label}</label>
      {:else}
        <p class="text-body-sm font-medium text-[var(--text)]">{label}</p>
      {/if}
      <p class="text-caption text-[var(--text-muted)] mt-0.5 leading-relaxed">{help}</p>
    </div>
    <div class="flex-1 min-w-0">{@render control()}</div>
  </div>
{/snippet}

{#snippet initialsTile()}
  <span class="relative grid place-items-center size-full">
    {#if identifier}
      <span class="text-body-sm font-mono font-semibold text-[var(--text-muted)]">{identifier.slice(0, 2)}</span>
    {:else}
      <ImagePlus size={18} class="text-[var(--text-faint)]" />
    {/if}
    {#if identifier}
      <span class="absolute -right-1.5 -bottom-1.5 size-5 grid place-items-center rounded-full bg-[var(--surface)] border border-[var(--border)] text-[var(--text-muted)]">
        <ImagePlus size={11} />
      </span>
    {/if}
  </span>
{/snippet}

<div class="flex flex-col gap-10">
  <!-- ── IDENTITY: the overview header, filled in where it will show ── -->
  <header class="flex items-start gap-4 sm:gap-5 pb-7 border-b border-[var(--border)]">
    <div class="shrink-0 pt-0.5">
      <IconPicker value={emoji} onchange={(v) => { emoji = v; }} size="lg" empty={initialsTile} />
    </div>
    <div class="flex-1 min-w-0">
      <label for="project-name" class="sr-only">Name</label>
      <!-- svelte-ignore a11y_autofocus -->
      <input
        id="project-name"
        type="text"
        bind:value={name}
        {placeholder}
        autofocus
        autocomplete="off"
        data-display-field
        class="block w-full bg-transparent border border-transparent text-[1.55rem] sm:text-[1.9rem] font-semibold tracking-tight leading-tight
               text-[var(--text)] placeholder:text-[var(--text-faint)] rounded-md px-2 -mx-2 py-1
               outline-none transition-colors hover:bg-[var(--chart-hover)] focus:bg-[var(--chart-hover)]"
      />
      <label for="project-desc" class="sr-only">Description</label>
      <textarea
        id="project-desc"
        bind:value={description}
        rows="1"
        placeholder="Add a description. What is this project about?"
        class="desc block w-full mt-1 bg-transparent border border-transparent text-body-lg leading-relaxed text-[var(--text-muted)]
               placeholder:text-[var(--text-faint)] rounded-md px-2 -mx-2 py-1 resize-none
               outline-none transition-colors hover:bg-[var(--chart-hover)] focus:bg-[var(--chart-hover)] focus:text-[var(--text)]"
      ></textarea>
    </div>
  </header>

  <!-- ── SETUP ─────────────────────────────────────────────────── -->
  <section aria-labelledby="new-setup-heading">
    <div class="mb-3">
      <h2 id="new-setup-heading" class="text-[1.15rem] font-semibold tracking-tight text-[var(--text)]">Setup</h2>
      <p class="text-body-sm text-[var(--text-muted)] mt-0.5">Only the identifier is needed. The rest can change later in settings.</p>
    </div>
    <div class="rounded-xl bg-[var(--surface)] shadow-[0_1px_2px_rgba(0,0,0,0.06)]">
      {#snippet idControl()}
        <div class="flex flex-wrap items-center gap-x-3 gap-y-1.5">
          <input
            id="project-id"
            type="text"
            value={identifier}
            oninput={onIdentifierInput}
            class="{fieldClass} w-[7.5rem] font-mono uppercase tracking-wider"
            placeholder="PRO"
            maxlength={IDENTIFIER_MAX}
            spellcheck="false"
            autocomplete="off"
            autocapitalize="characters"
            aria-invalid={identifierError ? "true" : undefined}
            aria-describedby="project-id-note"
          />
          <span id="project-id-note" class="text-caption" aria-live="polite">
            {#if identifierError}
              <span class="text-[var(--error)]">{identifierError}</span>
            {:else}
              <span class="text-[var(--text-muted)]">Issues become</span>
              <span class="font-mono font-semibold text-[var(--text)] px-1.5 py-0.5 rounded-md bg-[var(--chart-tile)]">{shownId}-1</span>
              <span class="font-mono text-[var(--text-faint)]">{shownId}-2 …</span>
            {/if}
          </span>
          {#if identifierTouched && suggested && identifier !== suggested}
            <button
              type="button"
              class="inline-flex items-center gap-1 text-caption text-[var(--text-muted)] hover:text-[var(--text)] rounded px-1 transition-colors"
              onclick={useSuggested}
            >
              <RotateCcw size={11} /> Use {suggested}
            </button>
          {/if}
        </div>
      {/snippet}
      {@render row("Identifier", `Up to ${IDENTIFIER_MAX} letters or digits, starting with a letter.`, idControl, "project-id")}

      {#snippet leadControl()}
        <div class="max-w-[36ch]">
          <Select
            options={userOptions}
            bind:value={leadUserId}
            placeholder="No lead"
            size="sm"
            renderSelected={selectedUser}
            renderOption={userOption}
          />
        </div>
      {/snippet}
      {@render row("Lead", "Owns the project and can change its settings.", leadControl)}

      {#if groups.length > 0}
        {#snippet groupControl()}
          <div class="max-w-[36ch]">
            <Select options={groupOptions} bind:value={groupId} placeholder="No folder" size="sm" />
          </div>
        {/snippet}
        <!-- Sidebar folder. Personal to the signed-in user, unlike Lead. -->
        {@render row("Folder", "Where it sits in your sidebar. Only you see this.", groupControl)}
      {/if}
    </div>
  </section>

  <!-- ── CONVENTIONS (LIF-524) ─────────────────────────────────── -->
  <section aria-labelledby="new-conventions-heading">
    <div class="mb-3">
      <h2 id="new-conventions-heading" class="text-[1.15rem] font-semibold tracking-tight text-[var(--text)]">Conventions</h2>
      <p class="text-body-sm text-[var(--text-muted)] mt-0.5">
        The rules for working in this tracker. Agents read them with their briefing at the start
        of each session. Pick a starting point; you can edit it or switch templates
        later on the project's Conventions tab.
      </p>
    </div>
    <ConventionsTemplatePicker {templates} bind:selected={conventions} excerpt />
  </section>
</div>

{#snippet selectedUser(opt: { value: string | number | null; label: string; [key: string]: unknown })}
  <div class="flex items-center gap-2">
    {#if opt.value !== null}
      <div
        class="size-5 rounded-full bg-[var(--accent)] text-[var(--accent-text)]
               flex items-center justify-center text-micro font-semibold shrink-0"
      >
        {userInitials(opt.label)}
      </div>
    {/if}
    <span class="text-body-sm {opt.value === null ? 'text-[var(--text-muted)]' : 'text-[var(--text)]'}">{opt.label}</span>
  </div>
{/snippet}

{#snippet userOption(opt: { value: string | number | null; label: string; [key: string]: unknown }, isSelected: boolean)}
  {#if opt.value === null}
    <span class="text-body text-[var(--text-faint)]">{opt.label}</span>
  {:else}
    <div class="flex items-center gap-2.5">
      <div
        class="size-7 rounded-full flex items-center justify-center
               text-micro font-semibold shrink-0
               {isSelected
          ? 'bg-[var(--accent)] text-[var(--accent-text)]'
          : 'bg-[var(--bg-subtle)] text-[var(--text-muted)]'}"
      >
        {userInitials(opt.label)}
      </div>
      <div class="min-w-0">
        <div class="flex items-center gap-1.5">
          <span
            class="text-body truncate
                   {isSelected ? 'text-[var(--accent)] font-medium' : 'text-[var(--text)]'}"
          >
            {opt.label}
          </span>
          {#if opt.is_admin}
            <span
              class="text-micro font-semibold uppercase tracking-wide
                     px-1 py-0.5 rounded bg-[var(--accent-subtle)] text-[var(--accent)]"
            >
              Admin
            </span>
          {/if}
        </div>
        <span class="text-caption text-[var(--text-faint)]">
          Member since {formatMemberSince(opt.created_at as string)}
        </span>
      </div>
    </div>
  {/if}
{/snippet}

<style>
  .desc {
    field-sizing: content;
    min-height: 2.25rem;
  }
</style>

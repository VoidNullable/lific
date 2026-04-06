<script lang="ts">
  import { listUsers, type UserSummary } from "./api";
  import IconPicker from "./IconPicker.svelte";
  import Select from "./Select.svelte";

  let {
    name = $bindable(""),
    identifier = $bindable(""),
    description = $bindable(""),
    emoji = $bindable(""),
    leadUserId = $bindable<number | null>(null),
    mode = "create",
    identifierLocked = false,
  }: {
    name?: string;
    identifier?: string;
    description?: string;
    emoji?: string;
    leadUserId?: number | null;
    mode?: "create" | "edit";
    identifierLocked?: boolean;
  } = $props();

  let users = $state<UserSummary[]>([]);
  let identifierTouched = $state(false);

  $effect(() => {
    listUsers().then((res) => {
      if (res.ok) users = res.data;
    });
  });

  // Auto-generate identifier from name (create mode only)
  $effect(() => {
    if (mode === "create" && !identifierTouched && name) {
      identifier = name
        .toUpperCase()
        .replace(/[^A-Z0-9]+/g, "")
        .slice(0, 5);
    }
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

  let previewId = $derived((identifier.trim().toUpperCase() || "PRO") + "-1");

  let userOptions = $derived([
    { value: null, label: "No lead" },
    ...users.map((u) => ({
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
</script>

<div class="max-w-[560px] mx-auto px-6 py-12 md:py-16">
  <!-- Name -->
  <div class="mb-8">
    <label
      for="project-name"
      class="block text-[0.8125rem] font-medium text-[var(--text)] mb-2 w-fit"
    >
      Name
    </label>
    <input
      id="project-name"
      type="text"
      bind:value={name}
      class="w-full rounded-md px-3 py-2.5 text-[0.9375rem]
             border border-[var(--border)] bg-[var(--bg-subtle)]
             text-[var(--text)] placeholder:text-[var(--text-faint)]
             outline-none transition-colors
             focus:border-[var(--accent)] focus:shadow-[0_0_0_3px_var(--accent-subtle)]"
      placeholder={mode === "create" ? placeholder : ""}
      autofocus={mode === "create"}
    />
  </div>

  <!-- Identifier + Lead + Icon row -->
  <div class="flex gap-5 items-start mb-8">
    <div class="shrink-0">
      <label
        for="project-id"
        class="block text-[0.8125rem] font-medium text-[var(--text)] mb-2 w-fit"
      >
        Identifier
      </label>
      <input
        id="project-id"
        type="text"
        bind:value={identifier}
        oninput={() => { identifierTouched = true; }}
        disabled={identifierLocked}
        class="w-[120px] rounded-md px-3 py-2.5 text-[0.9375rem] font-mono
               uppercase tracking-wide
               border border-[var(--border)] bg-[var(--bg-subtle)]
               text-[var(--text)] placeholder:text-[var(--text-faint)]
               outline-none transition-colors
               focus:border-[var(--accent)] focus:shadow-[0_0_0_3px_var(--accent-subtle)]
               disabled:opacity-50 disabled:cursor-not-allowed"
        placeholder="PRO"
        maxlength="5"
        spellcheck="false"
        autocapitalize="characters"
      />
      <p class="mt-1.5 text-[0.8125rem] text-[var(--text-faint)] w-fit">
        {mode === "edit" ? "Issue prefix" : "Issues become"}
      </p>
      <span
        class="inline-block font-mono text-[0.75rem] font-medium
               text-[var(--accent)] bg-[var(--accent-subtle)]
               px-1.5 py-0.5 rounded mt-0.5"
      >
        {previewId}
      </span>
    </div>

    <div class="flex-1">
      <label
        class="block text-[0.8125rem] font-medium text-[var(--text)] mb-2"
      >
        Lead
      </label>
      <Select
        options={userOptions}
        bind:value={leadUserId}
        placeholder="No lead"
        renderSelected={selectedUser}
        renderOption={userOption}
      />
    </div>

    <div>
      <label
        class="block text-[0.8125rem] font-medium text-[var(--text)] mb-2 w-fit"
      >
        Icon
      </label>
      <IconPicker value={emoji} onchange={(v) => { emoji = v; }} />
    </div>
  </div>

  <!-- Description -->
  <div class="mb-8">
    <div class="flex items-baseline gap-2 mb-2">
      <label
        for="project-desc"
        class="text-[0.8125rem] font-medium text-[var(--text)]"
      >
        Description
      </label>
      <span class="text-[0.75rem] text-[var(--text-faint)]">optional</span>
    </div>
    <textarea
      id="project-desc"
      bind:value={description}
      class="w-full rounded-md px-3 py-2.5 text-[0.9375rem] min-h-[100px]
             border border-[var(--border)] bg-[var(--bg-subtle)]
             text-[var(--text)] placeholder:text-[var(--text-faint)]
             outline-none resize-y transition-colors
             focus:border-[var(--accent)] focus:shadow-[0_0_0_3px_var(--accent-subtle)]"
      placeholder="What is this project about?"
      rows="3"
    ></textarea>
  </div>
</div>

{#snippet selectedUser(opt: { value: string | number | null; label: string; [key: string]: unknown })}
  <div class="flex items-center gap-2">
    {#if opt.value !== null}
      <div
        class="size-5 rounded-full bg-[var(--accent)] text-[var(--accent-text)]
               flex items-center justify-center text-[0.5625rem] font-semibold shrink-0"
      >
        {userInitials(opt.label)}
      </div>
    {/if}
    <span class="text-[0.9375rem] text-[var(--text)]">{opt.label}</span>
  </div>
{/snippet}

{#snippet userOption(opt: { value: string | number | null; label: string; [key: string]: unknown }, isSelected: boolean)}
  {#if opt.value === null}
    <span class="text-[0.875rem] text-[var(--text-faint)]">{opt.label}</span>
  {:else}
    <div class="flex items-center gap-2.5">
      <div
        class="size-7 rounded-full flex items-center justify-center
               text-[0.625rem] font-semibold shrink-0
               {isSelected
          ? 'bg-[var(--accent)] text-[var(--accent-text)]'
          : 'bg-[var(--bg-subtle)] text-[var(--text-muted)]'}"
      >
        {userInitials(opt.label)}
      </div>
      <div class="min-w-0">
        <div class="flex items-center gap-1.5">
          <span
            class="text-[0.875rem] truncate
                   {isSelected ? 'text-[var(--accent)] font-medium' : 'text-[var(--text)]'}"
          >
            {opt.label}
          </span>
          {#if opt.is_admin}
            <span
              class="text-[0.625rem] font-semibold uppercase tracking-wide
                     px-1 py-0.5 rounded bg-[var(--accent-subtle)] text-[var(--accent)]"
            >
              Admin
            </span>
          {/if}
        </div>
        <span class="text-[0.75rem] text-[var(--text-faint)]">
          Member since {formatMemberSince(opt.created_at as string)}
        </span>
      </div>
    </div>
  {/if}
{/snippet}

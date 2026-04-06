<script lang="ts">
  import { icons as lucideIcons } from "lucide";
  import { Icon as LucideIcon, type IconNode } from "lucide-svelte";
  import emojiData from "unicode-emoji-json";

  let {
    value = "",
    onchange,
  }: {
    value?: string;
    onchange?: (value: string) => void;
  } = $props();

  let open = $state(false);
  let tab = $state<"icons" | "emoji">("icons");
  let search = $state("");
  let searchEl = $state<HTMLInputElement | null>(null);
  let scrollContainer = $state<HTMLElement | null>(null);
  let scrollTop = $state(0);

  // ── Emoji data from unicode-emoji-json ─────────────
  // Pre-process into a flat searchable list, excluding skin tone variants and flags
  type EmojiEntry = { emoji: string; name: string; group: string };
  const ALL_EMOJIS: EmojiEntry[] = Object.entries(emojiData)
    .filter(([_, v]) => !v.skin_tone_support && v.group !== "Flags")
    .map(([emoji, v]) => ({ emoji, name: v.name, group: v.group }));

  // Group order for browsing
  const GROUP_ORDER = [
    "Objects", "Smileys & Emotion", "Animals & Nature",
    "Travel & Places", "Activities", "Food & Drink",
    "People & Body", "Symbols",
  ];
  const sortedEmojis = ALL_EMOJIS.sort((a, b) => {
    const ai = GROUP_ORDER.indexOf(a.group);
    const bi = GROUP_ORDER.indexOf(b.group);
    return (ai === -1 ? 99 : ai) - (bi === -1 ? 99 : bi);
  });

  // ── Lucide icons ─────────────────────────────────────
  const allIconNames = Object.keys(lucideIcons).sort();

  const POPULAR_ICONS = [
    "Folder", "FolderOpen", "Package", "Box", "Archive",
    "Code", "Terminal", "Braces", "FileCode", "GitBranch",
    "Bug", "Wrench", "Settings", "Cog", "Database",
    "Server", "Globe", "Shield", "Lock", "Key",
    "Palette", "PenTool", "Image", "Layout", "Eye",
    "Briefcase", "BarChart3", "DollarSign", "Users", "Building2",
    "Calendar", "Clock", "Timer", "Hourglass", "History",
    "Book", "BookOpen", "Notebook", "FileText", "Newspaper",
    "Rocket", "Lightbulb", "PuzzlePiece", "Trophy", "Flag",
    "Target", "Compass", "Map", "MapPin", "Navigation",
    "Mail", "MessageCircle", "Phone", "Bell", "Megaphone",
    "Music", "Video", "Camera", "Headphones", "Play",
    "Heart", "Star", "Zap", "Flame", "Sparkles",
    "Cloud", "Sun", "Moon", "Mountain", "Trees",
    "Leaf", "Flower2", "Atom", "FlaskConical", "Microscope",
    "Car", "Plane", "Ship", "Train", "Bike",
    "Home", "Store", "Hospital", "School", "Library",
    "Cpu", "Wifi", "Bluetooth", "Radio", "Satellite",
    "Gamepad2", "Dice5", "Crown", "Gem", "Coins",
  ];

  // ── Filtered results ─────────────────────────────────
  let filteredIcons = $derived.by(() => {
    if (!search.trim()) return allIconNames;
    const q = search.toLowerCase();
    return allIconNames.filter((name) => name.toLowerCase().includes(q));
  });

  let filteredEmojis = $derived.by(() => {
    if (!search.trim()) return sortedEmojis;
    const q = search.toLowerCase();
    return sortedEmojis.filter((e: EmojiEntry) => e.name.includes(q));
  });

  // ── Virtual scroll ───────────────────────────────────
  // Only kicks in when the result set overflows the viewport.
  const COLS = 8;
  const ROW_H = 36;
  const VIEWPORT_H = 280;
  const OVERSCAN = 2;

  function virtualRows(itemCount: number): {
    totalH: number; needsVirt: boolean;
    startIdx: number; endIdx: number; offsetY: number;
  } {
    const totalRows = Math.ceil(itemCount / COLS);
    const totalH = totalRows * ROW_H;
    if (totalH <= VIEWPORT_H) {
      // Fits without scrolling — render everything, no virtualization
      return { totalH, needsVirt: false, startIdx: 0, endIdx: itemCount, offsetY: 0 };
    }
    const startRow = Math.max(0, Math.floor(scrollTop / ROW_H) - OVERSCAN);
    const endRow = Math.min(totalRows, Math.ceil((scrollTop + VIEWPORT_H) / ROW_H) + OVERSCAN);
    return { totalH, needsVirt: true, startIdx: startRow * COLS, endIdx: endRow * COLS, offsetY: startRow * ROW_H };
  }

  let iconVirt = $derived(virtualRows(filteredIcons.length));
  let emojiVirt = $derived(virtualRows(filteredEmojis.length));

  // Reset scroll when search or tab changes
  $effect(() => {
    search; tab;
    scrollTop = 0;
    if (scrollContainer) scrollContainer.scrollTop = 0;
  });

  function onScroll(e: Event) {
    scrollTop = (e.target as HTMLElement).scrollTop;
  }

  // ── Actions ──────────────────────────────────────────
  function select(val: string) {
    onchange?.(val);
    open = false;
    search = "";
  }

  function toggle(e: Event) {
    e.stopPropagation();
    open = !open;
    if (open) {
      search = "";
      scrollTop = 0;
      requestAnimationFrame(() => searchEl?.focus());
    }
  }

  function handleWindowClick() {
    if (open) {
      open = false;
      search = "";
    }
  }

  // Parse current value
  let isLucide = $derived(value.startsWith("lucide:"));
  let lucideName = $derived(isLucide ? value.slice(7) : "");
  let iconNode = $derived(
    isLucide && lucideName in lucideIcons
      ? (lucideIcons as Record<string, IconNode>)[lucideName]
      : null
  );
</script>

<svelte:window onclick={handleWindowClick} />

<div class="relative">
  <!-- Trigger button -->
  <button
    class="size-10 rounded-lg border border-[var(--border)] bg-[var(--bg-subtle)]
           flex items-center justify-center text-[1.25rem]
           hover:border-[var(--accent)] transition-colors"
    onclick={toggle}
    title="Choose icon"
  >
    {#if value && isLucide && iconNode}
      <LucideIcon iconNode={iconNode} size={20} class="text-[var(--text)]" />
    {:else if value && !isLucide}
      {value}
    {:else}
      <span class="text-[var(--text-faint)] text-[0.875rem]">+</span>
    {/if}
  </button>

  <!-- Picker dropdown -->
  {#if open}
    <!-- svelte-ignore a11y_no_static_element_interactions a11y_click_events_have_key_events -->
    <div
      class="absolute left-0 top-full mt-2 z-30 w-[320px]
             bg-[var(--surface)] border border-[var(--border)]
             rounded-xl shadow-lg overflow-hidden"
      onclick={(e) => e.stopPropagation()}
    >
      <!-- Tabs -->
      <div class="flex border-b border-[var(--border)]">
        <button
          class="flex-1 text-[0.8125rem] py-2 transition-colors
                 {tab === 'icons'
            ? 'text-[var(--accent)] border-b-2 border-[var(--accent)] font-medium'
            : 'text-[var(--text-muted)] hover:text-[var(--text)]'}"
          onclick={() => { tab = "icons"; }}
        >
          Icons
        </button>
        <button
          class="flex-1 text-[0.8125rem] py-2 transition-colors
                 {tab === 'emoji'
            ? 'text-[var(--accent)] border-b-2 border-[var(--accent)] font-medium'
            : 'text-[var(--text-muted)] hover:text-[var(--text)]'}"
          onclick={() => { tab = "emoji"; }}
        >
          Emoji
        </button>
      </div>

      <!-- Search (always shown) -->
      <div class="p-2 border-b border-[var(--border)]">
        <input
          type="text"
          bind:value={search}
          bind:this={searchEl}
          class="w-full px-2.5 py-1.5 text-[0.8125rem] rounded-md
                 border border-[var(--border)] bg-[var(--bg)]
                 text-[var(--text)] placeholder:text-[var(--text-faint)]
                 outline-none focus:border-[var(--accent)]"
          placeholder={tab === "icons" ? "Search 1,900+ icons..." : "Search emojis..."}
        />
      </div>

      <!-- Virtualized grid -->
      <div
        class="overflow-y-auto"
        style="height: {Math.min(VIEWPORT_H, tab === 'icons' ? iconVirt.totalH + 16 : emojiVirt.totalH + 16)}px;"
        bind:this={scrollContainer}
        onscroll={onScroll}
      >
        {#if tab === "icons"}
          {#if filteredIcons.length === 0}
            <p class="text-[0.8125rem] text-[var(--text-faint)] text-center py-4">
              No icons match "{search}"
            </p>
          {:else if iconVirt.needsVirt}
            <!-- Virtualized: large result set -->
            <div class="relative p-2" style="height: {iconVirt.totalH}px;">
              <div
                class="absolute left-2 right-2 grid grid-cols-8 gap-0.5"
                style="top: {iconVirt.offsetY}px;"
              >
                {#each filteredIcons.slice(iconVirt.startIdx, iconVirt.endIdx) as iconName (iconName)}
                  {@const node = (lucideIcons as Record<string, IconNode>)[iconName]}
                  {#if node}
                    <button
                      class="size-9 rounded-md flex items-center justify-center
                             transition-colors hover:bg-[var(--bg-subtle)]
                             {value === `lucide:${iconName}` ? 'bg-[var(--accent-subtle)] text-[var(--accent)]' : 'text-[var(--text)]'}"
                      title={iconName}
                      onclick={() => select(`lucide:${iconName}`)}
                    >
                      <LucideIcon iconNode={node} size={18} />
                    </button>
                  {/if}
                {/each}
              </div>
            </div>
          {:else}
            <!-- Small set: render directly -->
            <div class="p-2 grid grid-cols-8 gap-0.5">
              {#each filteredIcons as iconName (iconName)}
                {@const node = (lucideIcons as Record<string, IconNode>)[iconName]}
                {#if node}
                  <button
                    class="size-9 rounded-md flex items-center justify-center
                           transition-colors hover:bg-[var(--bg-subtle)]
                           {value === `lucide:${iconName}` ? 'bg-[var(--accent-subtle)] text-[var(--accent)]' : 'text-[var(--text)]'}"
                    title={iconName}
                    onclick={() => select(`lucide:${iconName}`)}
                  >
                    <LucideIcon iconNode={node} size={18} />
                  </button>
                {/if}
              {/each}
            </div>
          {/if}
        {:else}
          {#if filteredEmojis.length === 0}
            <p class="text-[0.8125rem] text-[var(--text-faint)] text-center py-4">
              No emojis match "{search}"
            </p>
          {:else if emojiVirt.needsVirt}
            <!-- Virtualized: full emoji set -->
            <div class="relative p-2" style="height: {emojiVirt.totalH}px;">
              <div
                class="absolute left-2 right-2 grid grid-cols-8 gap-0.5"
                style="top: {emojiVirt.offsetY}px;"
              >
                {#each filteredEmojis.slice(emojiVirt.startIdx, emojiVirt.endIdx) as item (item.emoji)}
                  <button
                    class="size-9 rounded-md flex items-center justify-center text-[1.125rem]
                           transition-colors hover:bg-[var(--bg-subtle)]
                           {value === item.emoji ? 'bg-[var(--accent-subtle)]' : ''}"
                    title={item.name}
                    onclick={() => select(item.emoji)}
                  >
                    {item.emoji}
                  </button>
                {/each}
              </div>
            </div>
          {:else}
            <!-- Small filtered set: render directly -->
            <div class="p-2 grid grid-cols-8 gap-0.5">
              {#each filteredEmojis as item (item.emoji)}
                <button
                  class="size-9 rounded-md flex items-center justify-center text-[1.125rem]
                         transition-colors hover:bg-[var(--bg-subtle)]
                         {value === item.emoji ? 'bg-[var(--accent-subtle)]' : ''}"
                  title={item.name}
                  onclick={() => select(item.emoji)}
                >
                  {item.emoji}
                </button>
              {/each}
            </div>
          {/if}
        {/if}
      </div>

      <!-- Clear -->
      {#if value}
        <div class="border-t border-[var(--border)] p-2">
          <button
            class="w-full text-[0.8125rem] text-[var(--text-muted)] py-1.5
                   rounded-md hover:bg-[var(--bg-subtle)] transition-colors"
            onclick={() => select("")}
          >
            Remove icon
          </button>
        </div>
      {/if}
    </div>
  {/if}
</div>

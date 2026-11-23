<script lang="ts">
  // "Later": put an issue off until a day. Backed by a date wait, so it is
  // real, visible on the issue, and comes back on its own when the day
  // arrives. Days are the server's (the same clock waits use).
  import { Sun, CalendarDays, CalendarRange, CalendarClock } from "lucide-svelte";
  import { addDays, nextMonday } from "./inbox";

  let {
    today,
    onPick,
    onClose,
    align = "right",
  }: {
    /** Server day, YYYY-MM-DD. */
    today: string;
    onPick: (day: string) => void;
    onClose: () => void;
    align?: "left" | "right";
  } = $props();

  let custom = $state("");

  function weekday(day: string): string {
    const [y, m, d] = day.split("-").map(Number);
    return new Date(y, m - 1, d).toLocaleDateString(undefined, { weekday: "short", month: "short", day: "numeric" });
  }

  // On a Sunday, next Monday is tomorrow: offer the Monday after instead.
  const choices = $derived.by(() => {
    const tomorrow = addDays(today, 1);
    const monday = nextMonday(today);
    return [
      { label: "Tomorrow", day: tomorrow, icon: Sun },
      monday === tomorrow
        ? { label: "Monday after", day: nextMonday(monday), icon: CalendarDays }
        : { label: "Next Monday", day: monday, icon: CalendarDays },
      { label: "In a week", day: addDays(today, 7), icon: CalendarRange },
    ];
  });
</script>

<!-- svelte-ignore a11y_no_static_element_interactions a11y_click_events_have_key_events -->
<div
  class="popover-in absolute top-full mt-1.5 z-40 w-[232px] p-1 rounded-xl
         bg-[var(--surface)] border border-[var(--border)] shadow-lg text-body-sm
         {align === 'right' ? 'right-0 origin-top-right' : 'left-0 origin-top-left'}"
  role="menu"
  tabindex="-1"
  onclick={(e) => e.stopPropagation()}
  onkeydown={(e) => {
    if (e.key === "Escape") {
      e.stopPropagation();
      onClose();
    }
  }}
>
  <div class="px-2.5 pt-1.5 pb-1 text-micro uppercase tracking-widest font-semibold text-[var(--text-faint)]">
    Put off until
  </div>
  {#each choices as c (c.label)}
    <button
      role="menuitem"
      class="w-full flex items-center gap-2.5 px-2.5 h-8 rounded-md text-left
             text-[var(--text)] hover:bg-[var(--bg-subtle)] transition-colors"
      onclick={() => onPick(c.day)}
    >
      <c.icon size={14} class="text-[var(--text-muted)]" />
      <span class="flex-1">{c.label}</span>
      <span class="text-micro text-[var(--text-faint)]">{weekday(c.day)}</span>
    </button>
  {/each}
  <div class="my-1 mx-1 h-px bg-[var(--border)]"></div>
  <form
    class="flex items-center gap-1.5 px-1.5 py-1"
    onsubmit={(e) => {
      e.preventDefault();
      if (custom && custom > today) onPick(custom);
    }}
  >
    <CalendarClock size={14} class="ml-1 shrink-0 text-[var(--text-muted)]" />
    <input
      type="date"
      min={addDays(today, 1)}
      bind:value={custom}
      aria-label="Pick a day"
      class="flex-1 min-w-0 h-7 px-1.5 rounded-md text-caption bg-[var(--bg)]
             border border-[var(--border)] text-[var(--text)] outline-none
             focus:border-[var(--accent)]"
    />
    <button
      class="h-7 px-2 rounded-md text-caption font-medium text-[var(--accent-text)] bg-[var(--accent)]
             disabled:opacity-40"
      disabled={!custom || custom <= today}
    >
      Set
    </button>
  </form>
  <p class="px-2.5 pt-1 pb-1.5 text-micro leading-snug text-[var(--text-faint)]">
    Adds a date wait. It comes back to the inbox on that day.
  </p>
</div>

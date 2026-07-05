<script lang="ts" module>
  /**
   * TimeAgo (LIF-285) — a live-ticking relative timestamp.
   *
   * Renders `<time datetime title>{formatRelative(...)}</time>`. The text
   * re-derives from the shared module clock in now.svelte.ts, so every
   * TimeAgo on the page updates together (one timer for the whole app) and
   * "2m ago" never freezes. The `title` carries the full absolute date +
   * time so hovering any surface reveals the exact moment.
   *
   * Presentation only — all formatting lives in format.ts (single source of
   * truth). The <time> stays display:inline (no wrapper) so ticks never
   * shift layout.
   */
</script>

<script lang="ts">
  import { formatRelative, formatDateTime } from "./format";
  import { now } from "./now.svelte";

  let {
    date,
    class: klass = "",
  }: {
    // Call sites pass the same ISO strings they gave formatRelative.
    // Accept Date too for flexibility; normalize to the ISO string
    // format.ts expects (UTC without trailing "Z", which it re-appends).
    date: string | Date;
    class?: string;
  } = $props();

  // format.ts appends "Z" itself, so hand it a bare (no-"Z") ISO string.
  const iso = $derived(
    typeof date === "string" ? date : date.toISOString().replace(/Z$/, ""),
  );

  // Reading now() subscribes this derived to the shared tick; the value is
  // otherwise unused (formatRelative reads the wall clock internally). Void
  // it so lint doesn't flag an unused expression.
  const relative = $derived.by(() => {
    void now();
    return formatRelative(iso);
  });

  const full = $derived(formatDateTime(iso));
</script>

<time datetime={iso} title={full} class={klass}>{relative}</time>

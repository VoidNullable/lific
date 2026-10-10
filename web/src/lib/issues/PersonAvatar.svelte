<script lang="ts">
  // LIF-147: initials disc for a named assignee. Same vocabulary as the
  // member list and the sidebar's account avatar (accent fill, accent text).
  import { initials, personName, type Person } from "./assignees";

  let {
    person,
    size = "sm",
    ring = false,
  }: {
    person: Person;
    /** xs: list rows and cards. sm: sidebar and picker. */
    size?: "xs" | "sm";
    /** Separates overlapping avatars in a stack. */
    ring?: boolean;
  } = $props();

  const SIZE = {
    xs: "size-[18px] text-[8px]",
    sm: "size-6 text-micro",
  } as const;
</script>

<span
  class="{SIZE[size]} shrink-0 rounded-full grid place-items-center font-semibold tracking-wide
         select-none leading-none bg-[var(--accent)] text-[var(--accent-text)]
         {ring ? 'ring-2 ring-[var(--surface)]' : ''}"
  aria-hidden="true"
>
  {initials(personName(person))}
</span>

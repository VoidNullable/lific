import { icons } from "lucide";
import type { IconNode } from "lucide-svelte";
import emojiData from "unicode-emoji-json";

export function projectIcon(value: string | null | undefined): IconNode | string | null {
  if (!value) return null;
  if (value === "lific:logo") return value;
  if (value.startsWith("lucide:")) {
    const name = value.slice(7);
    return Object.hasOwn(icons, name)
      ? (icons as Record<string, IconNode>)[name]
      : null;
  }
  return Object.hasOwn(emojiData, value) ? value : null;
}

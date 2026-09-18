import type { AccentPreset } from "../theme";

/**
 * Swatch metadata for the accent picker (LIF-238). The swatch color is
 * the dark-mode --accent value for each preset — it's the more
 * saturated/vivid of the two and reads clearly as a small dot on
 * either a light or dark Settings page.
 *
 * Keep in sync with the `[data-accent="..."]` / `.dark[data-accent="..."]`
 * blocks in app.css — this is presentation-only metadata, not the
 * source of truth for the actual CSS variable values.
 */
export const ACCENT_PRESETS: {
  id: AccentPreset;
  label: string;
  swatch: string;
}[] = [
  { id: "indigo", label: "Indigo", swatch: "#9287d7" },
  { id: "teal", label: "Teal", swatch: "#4dd9c7" },
  { id: "rose", label: "Rose", swatch: "#f27a9c" },
  { id: "amber", label: "Amber", swatch: "#e0a530" },
  { id: "green", label: "Green", swatch: "#5cd192" },
  { id: "violet", label: "Violet", swatch: "#b48af0" },
];

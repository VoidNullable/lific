// Shared label color vocabulary (label management). Used by the ColorPicker,
// the inline create in LabelEditor, and the full LabelManager so a label's
// color is picked from one consistent, named, contrast-checked set wherever
// it's chosen.
//
// Accessibility (#8): every swatch is a ~600-weight hue, chosen so it stays
// legible as the *text* color of a tinted pill (color on a 10% tint of itself)
// in both light and dark themes. The old palette included pure yellow/amber
// (#EAB308 / #F59E0B) that washed out on light backgrounds — replaced with
// darker amber. Each color carries a human name for tooltips + aria-labels.

export type LabelColor = { name: string; value: string };

export const LABEL_PALETTE: LabelColor[] = [
  { name: "Red", value: "#EF4444" },
  { name: "Orange", value: "#F97316" },
  { name: "Amber", value: "#D97706" },
  { name: "Green", value: "#16A34A" },
  { name: "Emerald", value: "#059669" },
  { name: "Teal", value: "#0D9488" },
  { name: "Cyan", value: "#0891B2" },
  { name: "Blue", value: "#2563EB" },
  { name: "Indigo", value: "#4F46E5" },
  { name: "Violet", value: "#7C3AED" },
  { name: "Pink", value: "#DB2777" },
  { name: "Gray", value: "#6B7280" },
];

/** Just the hex values, in palette order. */
export const LABEL_COLORS: string[] = LABEL_PALETTE.map((c) => c.value);

/** Server's default label color (mirrors `default_label_color()` in Rust). */
export const DEFAULT_LABEL_COLOR = "#6B7280";

/** Human name for a hex (nearest by exact match, else "Custom"). */
export function colorName(hex: string): string {
  const found = LABEL_PALETTE.find(
    (c) => c.value.toLowerCase() === hex.toLowerCase(),
  );
  return found ? found.name : "Custom";
}

/** Deterministically pick a palette color from a string (so a freshly typed
 *  name gets a stable, varied default before the user overrides it). */
export function colorForName(name: string): string {
  let h = 0;
  for (let i = 0; i < name.length; i++) {
    h = (h * 31 + name.charCodeAt(i)) >>> 0;
  }
  // Skip the trailing gray so auto-assigned colors are lively; gray stays an
  // explicit choice.
  return LABEL_COLORS[h % (LABEL_COLORS.length - 1)];
}

/** Validate a 3- or 6-digit hex string (with or without leading #). */
export function isValidHex(s: string): boolean {
  return /^#?(?:[0-9a-fA-F]{3}|[0-9a-fA-F]{6})$/.test(s.trim());
}

/** Normalize user hex input to a "#rrggbb" string. Assumes isValidHex. */
export function normalizeHex(s: string): string {
  let h = s.trim().replace(/^#/, "");
  if (h.length === 3) h = h.split("").map((c) => c + c).join("");
  return "#" + h.toLowerCase();
}

/** Black or white, whichever reads better on the given solid hex background
 *  (WCAG relative-luminance threshold). For solid swatches/chips. */
export function readableTextColor(hex: string): string {
  const h = hex.replace(/^#/, "");
  const full = h.length === 3 ? h.split("").map((c) => c + c).join("") : h;
  const r = parseInt(full.slice(0, 2), 16) / 255;
  const g = parseInt(full.slice(2, 4), 16) / 255;
  const b = parseInt(full.slice(4, 6), 16) / 255;
  const lin = (c: number) => (c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4);
  const L = 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
  return L > 0.45 ? "#111827" : "#ffffff";
}

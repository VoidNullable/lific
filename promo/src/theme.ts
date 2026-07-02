/**
 * Lific brand tokens, lifted from web/src/app.css (dark mode).
 * Keep in sync with the app — the ad must look like the product.
 */
export const C = {
  // elevation tiers (sage-tinted near-black)
  bg: "#0d1110",
  bgSubtle: "#171c1a",
  surface: "#252c29",
  chrome: "#1c221f",
  // text ramp
  text: "#e3e8e6",
  textMuted: "#9ea9a3",
  textFaint: "#8a978f",
  border: "#3d4842",
  // accent (indigo-400 ramp)
  accent: "#9287d7",
  accentDeep: "#7566c0",
  accentSubtle: "#231e3a",
  // semantics
  success: "#4ade80",
  successDeep: "#3bb266",
  error: "#f87171",
  warn: "#fb923c",
  stone950: "#141210",
} as const;

/** Deliberately generic corporate blues for the cold-open "other tracker". */
export const GENERIC = {
  bg: "#10131a",
  surface: "#1a1f2b",
  border: "#2a3245",
  text: "#c6cdda",
  muted: "#69748c",
  blue: "#3b82f6",
} as const;

/**
 * Single swappable CTA. When lific.dev is live, change this one string
 * and re-render.
 */
export const CTA_URL = "github.com/VoidNullable/lific";

export const WIDTH = 1920;
export const HEIGHT = 1080;
export const FPS = 30;

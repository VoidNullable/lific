import { get, writable } from "svelte/store";

export type ThemePreference = "light" | "dark" | "system";
export type AccentPreset = "indigo" | "teal" | "rose" | "amber" | "green" | "violet";
export type Density = "comfortable" | "compact";
export type FontScale = "sm" | "md" | "lg";
export type MotionPreference = "system" | "reduced" | "full";

const STORAGE_KEY = "lific_theme";
const ACCENT_KEY = "lific_accent";
const DENSITY_KEY = "lific_density";
const FONT_SCALE_KEY = "lific_font_scale";
const MOTION_KEY = "lific_motion";

const ACCENTS: readonly AccentPreset[] = ["indigo", "teal", "rose", "amber", "green", "violet"];

function readPreference(): ThemePreference {
  const stored = typeof localStorage === "undefined" ? null : localStorage.getItem(STORAGE_KEY);
  if (stored === "light" || stored === "dark") return stored;
  return "system";
}

const preference = writable<ThemePreference>(readPreference());
const effectiveTheme = writable<"light" | "dark">(
  typeof window === "undefined" ? "light" : resolveTheme(get(preference)),
);

/** Shared writable preference. Direct store writes also persist and apply. */
export const themePreference = {
  subscribe: preference.subscribe,
  set: setPreference,
  update: (updater: (value: ThemePreference) => ThemePreference) => setPreference(updater(get(preference))),
};
/** Effective color scheme, including live OS changes while using system. */
export const resolvedTheme = { subscribe: effectiveTheme.subscribe };

export function getPreference(): ThemePreference {
  return get(preference);
}

/** Persist a preference. */
export function setPreference(pref: ThemePreference) {
  if (pref === "system") {
    localStorage.removeItem(STORAGE_KEY);
  } else {
    localStorage.setItem(STORAGE_KEY, pref);
  }
  preference.set(pref);
  apply(pref);
}

/** Resolve the effective theme (light or dark). */
export function resolveTheme(pref: ThemePreference): "light" | "dark" {
  if (pref === "system") {
    return window.matchMedia("(prefers-color-scheme: dark)").matches
      ? "dark"
      : "light";
  }
  return pref;
}

/** Apply the theme to the document. */
export function apply(pref: ThemePreference) {
  const resolved = resolveTheme(pref);
  document.documentElement.classList.toggle("dark", resolved === "dark");
  effectiveTheme.set(resolved);
}

/** Read stored accent preset, default to indigo. */
export function getAccent(): AccentPreset {
  const stored = localStorage.getItem(ACCENT_KEY);
  return ACCENTS.includes(stored as AccentPreset) ? (stored as AccentPreset) : "indigo";
}

/** Persist + apply an accent preset. */
export function setAccent(preset: AccentPreset) {
  if (preset === "indigo") {
    localStorage.removeItem(ACCENT_KEY);
  } else {
    localStorage.setItem(ACCENT_KEY, preset);
  }
  applyAccent(preset);
}

/** Apply the accent preset via `data-accent` on <html>. */
export function applyAccent(preset: AccentPreset) {
  document.documentElement.setAttribute("data-accent", preset);
}

/** Read stored density, default to comfortable. */
export function getDensity(): Density {
  return localStorage.getItem(DENSITY_KEY) === "compact" ? "compact" : "comfortable";
}

/** Persist + apply density. */
export function setDensity(density: Density) {
  if (density === "comfortable") {
    localStorage.removeItem(DENSITY_KEY);
  } else {
    localStorage.setItem(DENSITY_KEY, density);
  }
  applyDensity(density);
}

/** Apply density via a `density-compact` class on <html>. */
export function applyDensity(density: Density) {
  document.documentElement.classList.toggle("density-compact", density === "compact");
}

/** Read stored font scale, default to md. */
export function getFontScale(): FontScale {
  const stored = localStorage.getItem(FONT_SCALE_KEY);
  return stored === "sm" || stored === "lg" ? stored : "md";
}

/** Persist + apply font scale. */
export function setFontScale(scale: FontScale) {
  if (scale === "md") {
    localStorage.removeItem(FONT_SCALE_KEY);
  } else {
    localStorage.setItem(FONT_SCALE_KEY, scale);
  }
  applyFontScale(scale);
}

/** Apply font scale via `data-font-scale` on <html>. */
export function applyFontScale(scale: FontScale) {
  document.documentElement.setAttribute("data-font-scale", scale);
}

/** Read stored motion preference, default to system. */
export function getMotionPreference(): MotionPreference {
  const stored = localStorage.getItem(MOTION_KEY);
  return stored === "reduced" || stored === "full" ? stored : "system";
}

/** Persist + apply a motion preference. */
export function setMotionPreference(pref: MotionPreference) {
  if (pref === "system") {
    localStorage.removeItem(MOTION_KEY);
  } else {
    localStorage.setItem(MOTION_KEY, pref);
  }
  applyMotion(pref);
}

/** Resolve the effective (boolean) motion-reduced state for a preference. */
export function resolveMotion(pref: MotionPreference): boolean {
  if (pref === "reduced") return true;
  if (pref === "full") return false;
  return window.matchMedia("(prefers-reduced-motion: reduce)").matches;
}

/** Apply motion preference via `data-motion` on <html>. */
export function applyMotion(pref: MotionPreference) {
  document.documentElement.setAttribute("data-motion", resolveMotion(pref) ? "reduced" : "full");
}

/**
 * Live read of whether motion should currently be reduced (stored
 * preference resolved against the OS media query when "system").
 * For future JS-driven animations that need to check this at call
 * time rather than subscribing to the DOM attribute.
 */
export function motionReduced(): boolean {
  return resolveMotion(getMotionPreference());
}

let initialized = false;

/** Initialize on load + listen for system and other-tab changes. Idempotent. */
export function init() {
  preference.set(readPreference());
  apply(getPreference());
  applyAccent(getAccent());
  applyDensity(getDensity());
  applyFontScale(getFontScale());
  applyMotion(getMotionPreference());

  if (initialized) return;
  initialized = true;
  window.addEventListener("storage", (event) => {
    if (event.storageArea && event.storageArea !== localStorage) return;
    if (event.key === STORAGE_KEY || event.key === null) {
      const next = readPreference();
      preference.set(next);
      apply(next);
    }
  });

  window
    .matchMedia("(prefers-color-scheme: dark)")
    .addEventListener("change", () => {
      if (getPreference() === "system") {
        apply("system");
      }
    });

  window
    .matchMedia("(prefers-reduced-motion: reduce)")
    .addEventListener("change", () => {
      if (getMotionPreference() === "system") {
        applyMotion("system");
      }
    });
}

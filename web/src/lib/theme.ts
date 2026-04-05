export type ThemePreference = "light" | "dark" | "system";

const STORAGE_KEY = "lific_theme";

/** Read stored preference, default to system. */
export function getPreference(): ThemePreference {
  const stored = localStorage.getItem(STORAGE_KEY);
  if (stored === "light" || stored === "dark") return stored;
  return "system";
}

/** Persist a preference. */
export function setPreference(pref: ThemePreference) {
  if (pref === "system") {
    localStorage.removeItem(STORAGE_KEY);
  } else {
    localStorage.setItem(STORAGE_KEY, pref);
  }
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
}

/** Initialize on load + listen for system changes. */
export function init() {
  apply(getPreference());

  window
    .matchMedia("(prefers-color-scheme: dark)")
    .addEventListener("change", () => {
      if (getPreference() === "system") {
        apply("system");
      }
    });
}

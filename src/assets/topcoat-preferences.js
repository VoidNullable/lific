// Shared by private and public Topcoat pages; preferences never require auth.
export const STORAGE_KEY = "lific.topcoat.preferences";

export const PREFERENCE_VALUES = Object.freeze({
  theme: Object.freeze(["system", "light", "dark"]),
  accent: Object.freeze(["indigo", "teal", "rose", "amber", "green", "violet"]),
  density: Object.freeze(["comfortable", "compact"]),
  fontScale: Object.freeze(["small", "normal", "large"]),
});

export const DEFAULT_PREFERENCES = Object.freeze({
  theme: "system",
  accent: "indigo",
  density: "comfortable",
  fontScale: "normal",
});

export function normalizePreferences(value) {
  const input = value && typeof value === "object" && !Array.isArray(value) ? value : {};
  return Object.fromEntries(
    Object.entries(PREFERENCE_VALUES).map(([key, allowed]) => [
      key,
      Object.hasOwn(input, key) && allowed.includes(input[key])
        ? input[key]
        : DEFAULT_PREFERENCES[key],
    ]),
  );
}

function browserStorage() {
  try {
    return globalThis.localStorage;
  } catch {
    return undefined;
  }
}

function parsePreferences(json) {
  try {
    return normalizePreferences(JSON.parse(json));
  } catch {
    return normalizePreferences(null);
  }
}

export function loadPreferences(storage = browserStorage()) {
  try {
    return parsePreferences(storage?.getItem(STORAGE_KEY));
  } catch {
    return normalizePreferences(null);
  }
}

export function savePreferences(value, storage = browserStorage()) {
  const preferences = normalizePreferences(value);
  try {
    storage?.setItem(STORAGE_KEY, JSON.stringify(preferences));
  } catch {
    // Storage can be blocked or full; the current page still applies the choice.
  }
  return preferences;
}

function positionTooltip(tooltip, win) {
  const content = tooltip.querySelector?.(".tc-tooltip__content");
  const trigger = tooltip.querySelector?.("button");
  if (!content || !trigger) return;
  const viewport = win.visualViewport;
  const leftEdge = (viewport?.offsetLeft ?? 0) + 8;
  const topEdge = (viewport?.offsetTop ?? 0) + 8;
  const width = Math.max(1, (viewport?.width ?? win.innerWidth) - 16);
  const height = Math.max(1, (viewport?.height ?? win.innerHeight) - 16);
  const anchor = trigger.getBoundingClientRect();
  content.style.maxWidth = `min(18rem, ${width}px)`;
  content.style.maxHeight = `${height}px`;
  const above = Math.max(0, anchor.top - topEdge);
  const below = Math.max(0, topEdge + height - anchor.bottom);
  const placeAbove = content.getBoundingClientRect().height <= above || above > below;
  content.style.maxHeight = `${Math.max(1, placeAbove ? above : below)}px`;
  const rect = content.getBoundingClientRect();
  content.style.left = `${Math.max(leftEdge, Math.min(anchor.left, leftEdge + width - rect.width))}px`;
  const top = placeAbove ? anchor.top - rect.height : anchor.bottom;
  content.style.top = `${Math.max(topEdge, Math.min(top, topEdge + height - rect.height))}px`;
}

function repositionTooltips(doc, win) {
  for (const tooltip of doc.querySelectorAll(".tc-tooltip")) {
    if (tooltip.matches?.(":hover, :focus-within") && !tooltip.hasAttribute("data-dismissed")) {
      positionTooltip(tooltip, win);
    }
  }
}

export function applyPreferences(value, root = document.documentElement) {
  const preferences = normalizePreferences(value);
  Object.assign(root.dataset, preferences);
  for (const control of root.ownerDocument.querySelectorAll("[data-tc-preference]")) {
    const key = control.dataset.tcPreference;
    if (Object.hasOwn(preferences, key)) control.value = preferences[key];
  }
  if (root.ownerDocument.defaultView) {
    repositionTooltips(root.ownerDocument, root.ownerDocument.defaultView);
  }
  return preferences;
}

export function initializePreferences(doc = document, storage = browserStorage(), win = window) {
  let current = applyPreferences(loadPreferences(storage), doc.documentElement);
  doc.addEventListener("change", (event) => {
    const control = event.target.closest?.("[data-tc-preference]");
    const key = control?.dataset.tcPreference;
    if (!Object.hasOwn(PREFERENCE_VALUES, key) || !PREFERENCE_VALUES[key].includes(control.value)) {
      return;
    }
    current = applyPreferences(
      savePreferences({ ...current, [key]: control.value }, storage),
      doc.documentElement,
    );
  });
  win.addEventListener("storage", (event) => {
    if (event.storageArea !== storage || (event.key !== STORAGE_KEY && event.key !== null)) return;
    current = applyPreferences(parsePreferences(event.newValue), doc.documentElement);
  });
  const reposition = () => repositionTooltips(doc, win);
  win.addEventListener("resize", reposition);
  win.addEventListener("scroll", reposition, true);
  win.visualViewport?.addEventListener("resize", reposition);
  win.visualViewport?.addEventListener("scroll", reposition);

  doc.addEventListener("keydown", (event) => {
    if (event.key !== "Escape") return;
    for (const tooltip of doc.querySelectorAll(".tc-tooltip")) {
      tooltip.setAttribute("data-dismissed", "");
    }
  });
  for (const eventName of ["pointerover", "focusin"]) {
    doc.addEventListener(eventName, (event) => {
      const tooltip = event.target.closest?.(".tc-tooltip");
      if (tooltip && !tooltip.contains(event.relatedTarget)) {
        tooltip.removeAttribute("data-dismissed");
        positionTooltip(tooltip, win);
      }
    });
  }
}

if (typeof document !== "undefined" && typeof window !== "undefined") {
  initializePreferences();
}

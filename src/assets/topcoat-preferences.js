// Shared by private and public Topcoat pages; preferences never require auth.
export const STORAGE_KEY = "lific.topcoat.preferences";

export const PREFERENCE_VALUES = Object.freeze({
  theme: Object.freeze(["system", "light", "dark"]),
  accent: Object.freeze(["indigo", "teal", "rose", "amber", "green", "violet"]),
  density: Object.freeze(["comfortable", "compact"]),
  fontScale: Object.freeze(["small", "normal", "large"]),
  motion: Object.freeze(["system", "reduced", "full"]),
});

export const DEFAULT_PREFERENCES = Object.freeze({
  theme: "system",
  accent: "indigo",
  density: "comfortable",
  fontScale: "normal",
  motion: "system",
});

// Share the appearance keys with the existing frontend. The aggregate key
// above is accepted as a migration source for earlier Topcoat builds.
const PREFERENCE_KEYS = Object.freeze({
  theme: "lific_theme",
  accent: "lific_accent",
  density: "lific_density",
  fontScale: "lific_font_scale",
  motion: "lific_motion",
});
const FONT_SCALES = Object.freeze({ sm: "small", md: "normal", lg: "large" });

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
    const preferences = parsePreferences(storage?.getItem(STORAGE_KEY));
    for (const [name, key] of Object.entries(PREFERENCE_KEYS)) {
      const stored = storage?.getItem(key);
      if (stored === null || stored === undefined) continue;
      preferences[name] = name === "fontScale" ? FONT_SCALES[stored] : stored;
    }
    return normalizePreferences(preferences);
  } catch {
    return normalizePreferences(null);
  }
}

export function savePreferences(value, storage = browserStorage()) {
  const preferences = normalizePreferences(value);
  let persisted = true;
  for (const [name, preference] of Object.entries(preferences)) {
    persisted = persistPreference(name, preference, storage) && persisted;
  }
  if (persisted) {
    try {
      storage?.removeItem(STORAGE_KEY);
    } catch {
      // Keep the aggregate if migration cannot finish; it is the recovery copy.
    }
  }
  return preferences;
}

function persistPreference(name, value, storage) {
  const preference = normalizePreferences({ [name]: value })[name];
  const key = PREFERENCE_KEYS[name];
  try {
    if (preference === DEFAULT_PREFERENCES[name]) {
      storage?.removeItem(key);
    } else {
      const stored = name === "fontScale"
        ? Object.keys(FONT_SCALES).find(key => FONT_SCALES[key] === preference)
        : preference;
      storage?.setItem(key, stored);
    }
    return true;
  } catch {
    // A failed write does not stop the current page from applying the choice.
    return false;
  }
}

function savePreference(name, value, storage) {
  const preference = normalizePreferences({ [name]: value })[name];
  persistPreference(name, preference, storage);
  return preference;
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
  const win = root.ownerDocument.defaultView;
  const reducedMotion = win?.matchMedia?.("(prefers-reduced-motion: reduce)").matches === true;
  root.dataset.motion = preferences.motion === "system"
    ? reducedMotion ? "reduced" : "full"
    : preferences.motion;
  synchronizeControls(preferences, root.ownerDocument);
  if (win) repositionTooltips(root.ownerDocument, win);
  return preferences;
}

function synchronizeControls(preferences, doc) {
  for (const control of doc.querySelectorAll("[data-tc-preference]")) {
    const key = control.dataset.tcPreference;
    if (Object.hasOwn(preferences, key)) control.value = preferences[key];
  }
}

const initializedDocuments = new WeakMap();

export function initializePreferences(doc = document, storage = browserStorage(), win = window) {
  if (initializedDocuments.has(doc)) return initializedDocuments.get(doc);
  // Import earlier Topcoat preferences into the shared keys once. Subsequent
  // removal of a shared key then means "use the default", rather than falling
  // back to an obsolete aggregate value.
  let current = applyPreferences(
    savePreferences(loadPreferences(storage), storage),
    doc.documentElement,
  );
  const cleanups = [];
  const listen = (target, event, handler, options) => {
    if (!target) return;
    target.addEventListener(event, handler, options);
    cleanups.push(() => target.removeEventListener(event, handler, options));
  };
  listen(doc, "change", (event) => {
    const control = event.target.closest?.("[data-tc-preference]");
    const key = control?.dataset.tcPreference;
    if (!Object.hasOwn(PREFERENCE_VALUES, key) || !PREFERENCE_VALUES[key].includes(control.value)) {
      return;
    }
    current = applyPreferences(
      { ...current, [key]: savePreference(key, control.value, storage) },
      doc.documentElement,
    );
  });
  listen(win, "storage", (event) => {
    if (event.storageArea && event.storageArea !== storage) return;
    if (event.key !== null && event.key !== STORAGE_KEY && !Object.values(PREFERENCE_KEYS).includes(event.key)) return;
    const preferences = event.key === STORAGE_KEY && event.newValue !== null
      ? savePreferences(parsePreferences(event.newValue), storage)
      : loadPreferences(storage);
    current = applyPreferences(preferences, doc.documentElement);
  });
  listen(win.matchMedia?.("(prefers-reduced-motion: reduce)"), "change", () => {
    if (current.motion === "system") applyPreferences(current, doc.documentElement);
  });
  // Topcoat can insert preference controls after the document initialized.
  // Updating their values does not mutate attributes or trigger another pass.
  if (win.MutationObserver) {
    const observer = new win.MutationObserver(() => synchronizeControls(current, doc));
    observer.observe(doc.documentElement, { childList: true, subtree: true });
    cleanups.push(() => observer.disconnect());
  }
  const reposition = () => repositionTooltips(doc, win);
  listen(win, "resize", reposition);
  listen(win, "scroll", reposition, true);
  listen(win.visualViewport, "resize", reposition);
  listen(win.visualViewport, "scroll", reposition);

  listen(doc, "keydown", (event) => {
    if (event.key !== "Escape") return;
    for (const tooltip of doc.querySelectorAll(".tc-tooltip")) {
      tooltip.setAttribute("data-dismissed", "");
    }
  });
  for (const eventName of ["pointerover", "focusin"]) {
    listen(doc, eventName, (event) => {
      const tooltip = event.target.closest?.(".tc-tooltip");
      if (tooltip && !tooltip.contains(event.relatedTarget)) {
        tooltip.removeAttribute("data-dismissed");
        positionTooltip(tooltip, win);
      }
    });
  }
  const stop = () => {
    if (initializedDocuments.get(doc) !== stop) return;
    for (const cleanup of cleanups) cleanup();
    initializedDocuments.delete(doc);
  };
  initializedDocuments.set(doc, stop);
  return stop;
}

if (typeof document !== "undefined" && typeof window !== "undefined") {
  initializePreferences();
}

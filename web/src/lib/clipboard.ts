// LIF-284: one clipboard helper for the whole app so every "copy" button gives
// consistent feedback. Tries the async Clipboard API first; when that's absent
// (insecure context) or rejected (permissions), falls back to the legacy
// textarea + execCommand("copy") trick before giving up. Toasts on both paths:
// a short info toast on success, an error toast on failure — so a copy that
// silently fails (the old behavior at several call sites) can't happen anymore.

import { toast } from "./toast/toast.svelte";

export interface CopyOptions {
  /** Success-toast label: "Copied issue id" reads better than echoing a long
   *  value. Omit to echo the copied text. */
  label?: string;
  /** Suppress the success toast — for call sites that already show their own
   *  inline "Copied" feedback (a checkmark flip). The error toast still fires
   *  so a silent failure can't happen. Default false. */
  silentSuccess?: boolean;
}

/** Copy `text` to the clipboard and toast the outcome. `opts.label` customizes
 *  the success message; `opts.silentSuccess` suppresses the success toast for
 *  sites with their own inline flip. A string second arg is accepted as a
 *  shorthand for `{ label }`. Returns true on success so callers can gate an
 *  inline "copied" checkmark on the result. */
export async function copyToClipboard(
  text: string,
  opts?: string | CopyOptions,
): Promise<boolean> {
  const { label, silentSuccess = false } =
    typeof opts === "string" ? { label: opts } : (opts ?? {});
  const ok = await write(text);
  if (ok) {
    if (!silentSuccess)
      toast(`Copied ${label ?? text}`, { kind: "info", duration: 2500 });
  } else {
    toast("Couldn't copy to clipboard", { kind: "error" });
  }
  return ok;
}

/** Attempt the copy, returning whether it landed. Never throws. */
async function write(text: string): Promise<boolean> {
  if (navigator.clipboard?.writeText) {
    try {
      await navigator.clipboard.writeText(text);
      return true;
    } catch {
      // Fall through to the legacy path — some browsers reject the async API
      // in non-focused / insecure contexts but still honor execCommand.
    }
  }
  return legacyCopy(text);
}

/** The pre-Clipboard-API fallback: stage the text in an off-screen textarea,
 *  select it, and ask the document to copy the selection. */
function legacyCopy(text: string): boolean {
  try {
    const ta = document.createElement("textarea");
    ta.value = text;
    ta.setAttribute("readonly", "");
    ta.style.position = "absolute";
    ta.style.left = "-9999px";
    document.body.appendChild(ta);
    ta.select();
    const ok = document.execCommand("copy");
    document.body.removeChild(ta);
    return ok;
  } catch {
    return false;
  }
}

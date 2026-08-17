// LIF-418: the one entry point the upload pipeline calls to offer annotation.
//
// AnnotateDialog is mounted imperatively rather than being rendered by each
// composer. Two reasons: the prompt chip and the editor are viewport-fixed and
// have no business living inside a textarea's DOM subtree, and it keeps the
// integration surface at a single `await` inside `uploads.svelte.ts` instead of
// a component + state + wiring in every composer that ever grows an upload box.

import { mount, unmount } from "svelte";
import AnnotateDialog from "./AnnotateDialog.svelte";

/** Where a batch of files came from. Only the ones that plausibly carry a
 *  fresh screenshot get the annotate prompt; picking an existing file off disk
 *  does not, because you had every chance to edit it already. */
export type UploadSource = "paste" | "drop" | "camera" | "picker" | "voice";

const PROMPTED_SOURCES: ReadonlySet<UploadSource> = new Set(["paste", "drop", "camera"]);

/** Raster formats the canvas round-trip is safe for. GIF would lose its
 *  animation and SVG would either taint the canvas or rasterise a vector, so
 *  both pass straight through untouched. */
const ANNOTATABLE = new Set(["image/png", "image/jpeg", "image/jpg", "image/webp", "image/bmp"]);

export function isAnnotatable(file: File): boolean {
  return ANNOTATABLE.has(file.type.toLowerCase());
}

/** Mount the prompt/editor for one file and resolve with whatever should be
 *  uploaded: the annotated copy, or the original if it was skipped. */
export function annotate(file: File): Promise<File> {
  return new Promise((resolve) => {
    const target = document.createElement("div");
    document.body.appendChild(target);
    let done = false;
    const app = mount(AnnotateDialog, {
      target,
      props: {
        file,
        onDone(result: File) {
          if (done) return;
          done = true;
          resolve(result);
          // Detach on a later turn so the resolving handler is not unmounted
          // out from under itself mid-call.
          queueMicrotask(() => {
            void unmount(app);
            target.remove();
          });
        },
      },
    });
  });
}

/**
 * Offer annotation for every eligible image in a batch, sequentially, and
 * return the files to actually upload.
 *
 * Sequential because two prompt chips stacked on top of each other is
 * nonsense; pasting three screenshots walks you through them one at a time.
 * Anything not eligible (a PDF, a picked file, a server-side render with no
 * `document`) is returned untouched, so callers can pipe every upload through
 * this without branching.
 */
export async function maybeAnnotate(files: File[], source: UploadSource): Promise<File[]> {
  if (typeof document === "undefined" || !PROMPTED_SOURCES.has(source)) return files;
  const out: File[] = [];
  for (const file of files) {
    out.push(isAnnotatable(file) ? await annotate(file) : file);
  }
  return out;
}

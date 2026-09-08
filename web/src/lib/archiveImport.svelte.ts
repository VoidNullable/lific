import { ARCHIVE_UNKNOWN, importProjectArchive, onSessionChange, type ArchiveImportResult } from "./api";

const pendingKey = (userId: number) => `lific:archive-import-pending:${userId}`;
let owner: number | null = null;
let session: string | null = null;
let generation = 0;
let watchingSession = false;

export const archiveImport = $state({
  phase: "idle" as "idle" | "uploading" | "processing" | "success" | "error" | "unknown",
  progress: null as number | null,
  error: "",
  result: null as ArchiveImportResult | null,
});

// Keep an in-flight transfer across route changes, never across identities.
export function hideArchiveImport() {
  generation++;
  owner = null; session = null;
  Object.assign(archiveImport, { phase: "idle", progress: null, error: "", result: null });
}

export function restoreArchiveImport(userId: number, token: string) {
  if (!Number.isSafeInteger(userId) || userId <= 0 || token !== localStorage.getItem("lific_token")) return;
  if (!watchingSession) {
    onSessionChange(hideArchiveImport);
    watchingSession = true;
  }
  if (owner === userId && session === token) return;
  owner = userId; session = token;
  generation++;
  const pending = sessionStorage.getItem(pendingKey(userId)) === "1";
  Object.assign(archiveImport, {
    phase: pending ? "unknown" : "idle",
    progress: null, error: pending ? ARCHIVE_UNKNOWN : "", result: null,
  });
}

export async function startArchiveImport(file: File) {
  if (owner === null || !session || session !== localStorage.getItem("lific_token")) return;
  if (["uploading", "processing", "unknown", "success"].includes(archiveImport.phase)) return;
  const current = ++generation;
  const userId = owner, token = session;
  const key = pendingKey(userId);
  const live = () => current === generation && owner === userId && session === token && token === localStorage.getItem("lific_token");
  archiveImport.phase = "uploading";
  archiveImport.progress = 0;
  archiveImport.error = "";
  sessionStorage.setItem(key, "1");
  const result = await importProjectArchive(file,
    (progress) => { if (live()) archiveImport.progress = progress; },
    () => { if (live()) archiveImport.phase = "processing"; });
  if (!live()) return;
  if (result.ok) {
    sessionStorage.removeItem(key);
    archiveImport.result = result.data;
    archiveImport.phase = "success";
  } else {
    archiveImport.error = result.error;
    archiveImport.phase = result.status === null ? "unknown" : "error";
    if (result.status !== null) sessionStorage.removeItem(key);
  }
}

export function resetArchiveImport() {
  if (owner === null || session !== localStorage.getItem("lific_token")) return;
  if (["uploading", "processing"].includes(archiveImport.phase)) return;
  generation++;
  sessionStorage.removeItem(pendingKey(owner));
  Object.assign(archiveImport, { phase: "idle", progress: null, error: "", result: null });
}

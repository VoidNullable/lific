// LIF-418: pure helpers behind the project Files view.
//
// Everything here is a plain function over plain data so it can be unit
// tested (web/tests/files.test.ts) without mounting a component: the filter
// chip set, the sweep countdown wording, the delete-confirm sentence, the
// client-side permission mirror, and the route a linked-entity chip navigates
// to. Files.svelte holds only rendering and fetching.

import type { LinkedEntity, MimeClass, ProjectAttachment } from "../api";

/** The filter chips, in display order. `null` is the "All" chip. */
export const MIME_FILTERS: { value: MimeClass | null; label: string }[] = [
  { value: null, label: "All" },
  { value: "image", label: "Images" },
  { value: "video", label: "Video" },
  { value: "audio", label: "Audio" },
  { value: "text", label: "Text" },
  { value: "pdf", label: "PDF" },
  { value: "archive", label: "Archives" },
  { value: "other", label: "Other" },
];

export const SORT_OPTIONS: {
  value: "created_at" | "size" | "filename";
  label: string;
}[] = [
  { value: "created_at", label: "Newest first" },
  { value: "size", label: "Largest first" },
  { value: "filename", label: "Name A to Z" },
];

/** Human label for a class, used by the row subtitle and the empty state. */
export function mimeClassLabel(cls: MimeClass): string {
  return MIME_FILTERS.find((f) => f.value === cls)?.label ?? "Other";
}

/**
 * The countdown shown next to a pending orphan.
 *
 * Deliberately coarse: the sweeper runs hourly, so minute-level precision
 * would be a lie. Zero (or less) means the file is already past the grace
 * window and goes on the next pass.
 */
export function formatSweepCountdown(seconds: number): string {
  if (seconds <= 0) return "swept on the next pass";
  const hours = Math.floor(seconds / 3600);
  if (hours >= 24) {
    const days = Math.floor(hours / 24);
    return `swept in ${days} day${days === 1 ? "" : "s"}`;
  }
  if (hours >= 1) return `swept in ${hours}h`;
  const minutes = Math.max(1, Math.floor(seconds / 60));
  return `swept in ${minutes} min`;
}

/**
 * The inline confirm sentence. States the blast radius, because deleting a
 * file that three issues embed breaks all three, and the row only shows the
 * chips for one project.
 */
export function deleteConfirmMessage(referenceCount: number): string {
  if (referenceCount === 0) {
    return "Removes the file. It has no references.";
  }
  return `Removes the file and its ${referenceCount} reference${
    referenceCount === 1 ? "" : "s"
  }.`;
}

/**
 * Client-side mirror of the server's delete gate (uploader, a project
 * maintainer, or an admin). This only hides a button; the server is still the
 * boundary, and it answers 403 either way.
 *
 * Fails open when the viewer is unknown (`viewerId === null`, i.e. the `me()`
 * probe hasn't resolved or failed): showing a button that might 403 beats
 * hiding one that would have worked, matching how projectRole.svelte.ts
 * treats an unresolved role.
 */
export function canDeleteAttachment(input: {
  uploaderId: number | null;
  viewerId: number | null;
  isAdmin: boolean;
  canEdit: boolean;
}): boolean {
  if (input.isAdmin || input.canEdit) return true;
  if (input.viewerId === null) return true;
  return input.uploaderId !== null && input.uploaderId === input.viewerId;
}

/**
 * The hash route a linked-entity chip navigates to, or null when the link
 * can't be resolved to a page in this UI (a workspace page has no project
 * route, and a comment on one inherits that).
 */
export function entityHref(
  projectIdentifier: string,
  entity: LinkedEntity,
): string | null {
  if (entity.page_id !== null) {
    return `/${projectIdentifier}/pages/${entity.page_id}`;
  }
  if (entity.identifier) {
    return `/${projectIdentifier}/issues/${entity.identifier}`;
  }
  return null;
}

/** Chip label: the identifier, with a marker when the reference is a comment
 *  rather than the entity body. */
export function entityChipLabel(entity: LinkedEntity): string {
  const base = entity.identifier ?? "unlinked";
  return entity.entity_type === "comment" ? `${base} (comment)` : base;
}

/** The distinct uploader usernames present in a set of rows, sorted, for the
 *  uploader filter dropdown. */
export function uploaderOptions(items: ProjectAttachment[]): string[] {
  const names = new Set<string>();
  for (const item of items) {
    if (item.uploader) names.add(item.uploader);
  }
  return [...names].sort((a, b) => a.localeCompare(b));
}

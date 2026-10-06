import type { Issue, RelationType } from "../api";

/** One relation group as seen from the issue being viewed. The API stores
 *  `blocks` and `duplicate` as source→target edges, so the inverse groups
 *  ("Blocked by", "Duplicated by") are created with the other issue as source. */
export interface RelationKind {
  field: "blocked_by" | "blocks" | "relates_to" | "duplicates" | "duplicated_by";
  label: string;
  type: RelationType;
  inverse: boolean;
}

export const RELATION_KINDS: RelationKind[] = [
  { field: "blocked_by", label: "Blocked by", type: "blocks", inverse: true },
  { field: "blocks", label: "Blocks", type: "blocks", inverse: false },
  { field: "relates_to", label: "Related", type: "relates_to", inverse: false },
  { field: "duplicates", label: "Duplicate of", type: "duplicate", inverse: false },
  { field: "duplicated_by", label: "Duplicated by", type: "duplicate", inverse: true },
];

export function linkRequest(kind: RelationKind, self: string, other: string) {
  return kind.inverse
    ? { source: other, target: self, relation_type: kind.type }
    : { source: self, target: other, relation_type: kind.type };
}

/** Deduplicated: `relates_to` is matched in both directions, so mirrored
 *  rows (A→B and B→A) would otherwise list the same issue twice. */
export function relationsOf(issue: Issue, kind: RelationKind): string[] {
  return [...new Set(issue[kind.field] ?? [])];
}

export function hasRelations(issue: Issue): boolean {
  return RELATION_KINDS.some(({ field }) => (issue[field]?.length ?? 0) > 0);
}

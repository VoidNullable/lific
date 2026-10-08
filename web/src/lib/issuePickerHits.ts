// Ordering and labelling for IssuePickerModal results (LIF-504).
//
// The picker merges up to three sources: a typed identifier resolved
// directly, a search scoped to the current project, and (when the caller
// opts into cross-project search) a search over every project the user can
// see. Kept free of Svelte and the network so the ordering is testable.

export type PickerHit = {
  id: number;
  identifier: string;
  title: string;
  status?: string;
  projectId: number | null;
  /** Set on hits from a project other than the picker's own. */
  projectLabel?: string;
};

/** The parts of a search result the picker reads. */
export type PickerSearchRow = {
  result_type: string;
  id: number;
  identifier: string | null;
  title: string;
  project_id: number | null;
};

export type PickerProject = { id: number; identifier: string; name: string };

export type MergeInput = {
  projectId: number;
  crossProject: boolean;
  /** An issue resolved from an identifier-shaped query, if any. */
  idHit?: {
    id: number;
    identifier: string;
    title: string;
    status?: string;
    project_id: number;
  } | null;
  /** Search hits scoped to the current project. */
  scoped?: PickerSearchRow[] | null;
  /** Search hits across every visible project (cross-project mode only). */
  global?: PickerSearchRow[] | null;
  /** Visible projects, used to label hits from other projects. */
  projects?: PickerProject[];
};

/** "Game (GAME)" when the name differs from the code, else just the code. */
export function projectLabel(
  projectId: number | null,
  identifier: string,
  projects: PickerProject[] = [],
): string {
  const project = projects.find((p) => p.id === projectId);
  const code = project?.identifier ?? identifier.split("-")[0];
  if (!project || !project.name || project.name === code) {
    return code;
  }
  return `${project.name} (${code})`;
}

/**
 * Merge picker sources into one list. An exact identifier match leads,
 * because the user named that issue outright. Then come current-project
 * hits, then (in cross-project mode) hits from other projects, each
 * labelled with its project. Without `crossProject`, hits from other
 * projects in the search results are dropped, as before.
 */
export function mergePickerHits(input: MergeInput): PickerHit[] {
  const { projectId, crossProject, projects = [] } = input;
  const out: PickerHit[] = [];
  const seen = new Set<string>();
  const label = (pid: number | null, identifier: string) =>
    crossProject && pid !== null && pid !== projectId
      ? projectLabel(pid, identifier, projects)
      : undefined;

  const idHit = input.idHit;
  if (idHit) {
    out.push({
      id: idHit.id,
      identifier: idHit.identifier,
      title: idHit.title,
      status: idHit.status,
      projectId: idHit.project_id,
      projectLabel: label(idHit.project_id, idHit.identifier),
    });
    seen.add(idHit.identifier);
  }

  const take = (rows: PickerSearchRow[] | null | undefined, foreign: boolean) => {
    for (const r of rows ?? []) {
      if (r.result_type !== "issue" || !r.identifier) continue;
      if (seen.has(r.identifier)) continue;
      const isCurrent = r.project_id === null || r.project_id === projectId;
      if (isCurrent === foreign) continue;
      out.push({
        id: r.id,
        identifier: r.identifier,
        title: r.title,
        projectId: r.project_id,
        projectLabel: label(r.project_id, r.identifier),
      });
      seen.add(r.identifier);
    }
  };

  // Current project first. The cross-project search may also carry
  // current-project hits the scoped one ranked out; keep those too.
  take(input.scoped, false);
  if (crossProject) {
    take(input.global, false);
    take(input.global, true);
  }
  return out;
}

function isForeign(hit: PickerHit, projectId: number): boolean {
  return hit.projectId !== null && hit.projectId !== projectId;
}

/** Whether the picker draws an "Other projects" divider above hit `i`: the
 *  first hit from another project that follows a current-project hit. */
export function startsForeignGroup(hits: PickerHit[], i: number, projectId: number): boolean {
  return i > 0 && isForeign(hits[i], projectId) && !isForeign(hits[i - 1], projectId);
}

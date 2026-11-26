// LIF-234 — role-aware UI affordance gating.
//
// A module singleton (mirrors issues/peek.svelte.ts and toast.svelte.ts):
// one shared, cached read of the caller's effective role on the current
// project, consumed by every route/component that shows a mutate control —
// so we DON'T sprinkle an ad-hoc `getMyProjectRole` fetch into IssueList,
// the board, IssueDetail, PageDetail, ProjectSettings, etc.
//
// The store answers "what can I do on THIS project," not "who's on it":
//   - canEdit   — may create/edit/delete content + drag on the board
//                 (role >= maintainer, or admin).
//   - canManage — may touch project settings / danger zone / members /
//                 import (role === lead, or admin).
//
// Every instance enforces project roles (LIF-518; see src/authz.rs), so the
// derivation below always gates a non-admin by their role.
//
// ## Fail-open on load error
//
// The gates only ever HIDE/DISABLE affordances — they're UX polish, never
// the security boundary (the server still returns clean 403s). So if the
// role fetch fails for a reason other than a definitive "you're not a
// member" 403 (network blip, etc.), we fail OPEN: keep the UI interactive
// rather than locking a legitimate user out of buttons that would actually
// work. A member who is genuinely a viewer gets the correct gated view; a
// transient error just briefly shows the full UI, and any real mutation is
// still safely denied server-side.

import { getMyProjectRole, me, type ProjectRole } from "./api";
import { isSignedOut, onSignedOutChange } from "./signedOut";

/** The inputs the pure derivation needs. Kept separate from the reactive
 *  store so the derivation can be unit-tested without runes. */
export interface RoleInputs {
  role: ProjectRole | null;
  isAdmin: boolean;
}

/** May the user create/edit/delete content (issues, pages, plans, modules,
 *  labels) and drag on the board? True when the user is a workspace admin or
 *  their role is maintainer-or-higher. */
export function deriveCanEdit(i: RoleInputs): boolean {
  if (i.isAdmin) return true;
  return i.role === "maintainer" || i.role === "lead";
}

/** May the user manage the project itself — settings, danger zone, member
 *  management, import? True when the user is a workspace admin or their role
 *  is lead. */
export function deriveCanManage(i: RoleInputs): boolean {
  if (i.isAdmin) return true;
  return i.role === "lead";
}

/**
 * May the user publish this project to the open internet (LIF-465)? The
 * server gate is `require_role(.., Lead)`. Unlike `canManage` it doesn't fail
 * open, and `isLead` counts because a project whose lead predates LIF-195 can
 * have `lead_user_id` set without a `project_members` row.
 */
export function deriveCanPublish(i: RoleInputs & { isLead?: boolean }): boolean {
  return i.isAdmin || i.role === "lead" || i.isLead === true;
}

/** May the user comment? Comments are Viewer-gated server-side (LIF-197),
 *  so ANY project member (viewer+) can comment, as can workspace admins. The
 *  only case that can't is a non-member, but they can't load the surface at
 *  all (the read is itself Viewer-gated), so in practice this is true
 *  wherever a document renders. Kept explicit so the composer stays enabled
 *  even when `canEdit` is false (the exact viewer case LIF-234 calls out). */
export function deriveCanComment(i: RoleInputs): boolean {
  if (i.isAdmin) return true;
  return i.role === "viewer" || i.role === "maintainer" || i.role === "lead";
}

/** The read-only banner blurb, or null when the user isn't gated. Shown by
 *  detail toolbars so a viewer understands *why* the editors are gone. */
export function readOnlyReason(i: RoleInputs): string | null {
  if (deriveCanEdit(i)) return null;
  return "Read-only — you're a viewer on this project.";
}

class ProjectRoleState {
  /** The project this state currently describes; null before any load. */
  projectId = $state<number | null>(null);
  role = $state<ProjectRole | null>(null);
  isAdmin = $state(false);
  /** False until a role answer for `projectId` arrives, and again when the
   *  read fails. Every gate stays open meanwhile (see "Fail-open" above). */
  known = $state(false);
  /** True while a fetch is in flight for the current project. */
  loading = $state(false);
  /** True once at least one load for `projectId` has resolved, so consumers
   *  can avoid flashing the gated view before we know the answer. */
  loaded = $state(false);

  /** The signed-in user's workspace-admin flag, learned once via `me()`.
   *  `null` = not yet learned. Workspace pages are admin-only, mirroring the
   *  server (authz::require_workspace_admin). */
  meIsAdmin = $state<boolean | null>(null);

  /** May the user edit a WORKSPACE (project-less) page? Admin-only, exactly
   *  as the server enforces; open before we've learned the flag (fail-open). */
  get canEditWorkspacePage(): boolean {
    return this.meIsAdmin !== false;
  }

  private inputs(): RoleInputs {
    return { role: this.role, isAdmin: this.isAdmin };
  }

  get canEdit(): boolean {
    return !this.known || deriveCanEdit(this.inputs());
  }
  get canManage(): boolean {
    return !this.known || deriveCanManage(this.inputs());
  }
  get canComment(): boolean {
    return !this.known || deriveCanComment(this.inputs());
  }
  get readOnly(): boolean {
    return !this.canEdit;
  }
  get readOnlyReason(): string | null {
    return this.known ? readOnlyReason(this.inputs()) : null;
  }
}

export const projectRole = new ProjectRoleState();

// Simple per-project-id cache so switching back and forth between projects
// (or two components on the same route both asking) doesn't refetch. A role
// change is rare and re-applied on the next project switch / reload; that's
// an acceptable staleness window for pure UI gating.
const cache = new Map<number, RoleInputs>();
// Tracks the in-flight request's project so a slow response for a project
// we've since navigated away from can't clobber the current one.
let inFlightFor: number | null = null;

/** Ensure the store reflects `projectId`, loading it once (from cache when
 *  possible). Call this on project switch (e.g. from Layout, or any
 *  project-scoped route's load). Safe to call repeatedly — it no-ops when
 *  the store is already on this project and loaded, and dedupes concurrent
 *  callers via the cache + in-flight guard. */
export async function loadProjectRole(projectId: number): Promise<void> {
  // LIF-583: a signed-out reader has no role anywhere. The server answers
  // the same (role null), so apply it without asking, and never fail open
  // the way a signed-in load does below.
  if (isSignedOut()) {
    apply(projectId, { role: null, isAdmin: false });
    return;
  }
  // Already showing this project's answer.
  if (projectRole.projectId === projectId && projectRole.loaded) return;

  const cached = cache.get(projectId);
  if (cached) {
    apply(projectId, cached);
    return;
  }

  // New project: reset visible state so a component doesn't read a stale
  // previous-project role during the fetch. Default to fail-open
  // (known=false ⇒ canEdit/canManage true) until we hear otherwise.
  projectRole.projectId = projectId;
  projectRole.role = null;
  projectRole.known = false;
  projectRole.isAdmin = false;
  projectRole.loaded = false;
  projectRole.loading = true;
  inFlightFor = projectId;

  const res = await getMyProjectRole(projectId);

  // A newer load for a different project started while we awaited — drop
  // this result so we don't overwrite the current project's state.
  if (inFlightFor !== projectId) return;
  inFlightFor = null;
  if (res.ok) {
    const inputs: RoleInputs = { role: res.data.role, isAdmin: res.data.is_admin };
    cache.set(projectId, inputs);
    apply(projectId, inputs);
  } else {
    // Fail open (see module doc): a fetch failure must not strand a
    // legitimate user without buttons. We don't cache this so a later
    // navigation retries.
    projectRole.loading = false;
    projectRole.loaded = true;
  }
}

function apply(projectId: number, inputs: RoleInputs): void {
  projectRole.projectId = projectId;
  projectRole.role = inputs.role;
  projectRole.isAdmin = inputs.isAdmin;
  projectRole.known = true;
  projectRole.loading = false;
  projectRole.loaded = true;
}

// Signing in or out changes who is asking, so every remembered answer goes:
// the visible one, the per-project cache, and the admin flag. Clearing
// `inFlightFor` drops a load that was started by the other audience.
onSignedOutChange(() => {
  cache.clear();
  projectRole.loaded = false;
  projectRole.projectId = null;
  projectRole.meIsAdmin = null;
  inFlightFor = null;
});

/** Learn the signed-in user's workspace-admin flag once (cached), for
 *  gating workspace/project-less pages. Cheap and idempotent — no-ops after
 *  the first successful read. */
export async function ensureMeAdmin(): Promise<void> {
  if (projectRole.meIsAdmin !== null || isSignedOut()) return;
  const res = await me();
  if (res.ok) projectRole.meIsAdmin = res.data.is_admin;
}

/** Drop the cached role for one project (e.g. after the current user's
 *  membership changes in ProjectMembers) so the next load refetches. */
export function invalidateProjectRole(projectId: number): void {
  cache.delete(projectId);
  if (projectRole.projectId === projectId) projectRole.loaded = false;
}

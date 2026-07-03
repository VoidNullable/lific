const BASE = "/api";

export interface AuthUser {
  id: number;
  username: string;
  email: string;
  display_name: string;
  is_admin: boolean;
}

export interface AuthResponse {
  user: AuthUser;
  token: string;
  expires_at: string;
}

export interface ApiError {
  error: string;
}

async function request<T>(
  path: string,
  options: RequestInit = {}
): Promise<{ ok: true; data: T } | { ok: false; error: string }> {
  const token = localStorage.getItem("lific_token");
  const headers: Record<string, string> = {
    "Content-Type": "application/json",
    ...(options.headers as Record<string, string>),
  };

  if (token) {
    headers["Authorization"] = `Bearer ${token}`;
  }

  try {
    const res = await fetch(`${BASE}${path}`, { ...options, headers });
    const body = await res.json();

    if (!res.ok) {
      return { ok: false, error: body.error || `HTTP ${res.status}` };
    }

    return { ok: true, data: body as T };
  } catch (e) {
    return {
      ok: false,
      error: "Couldn't reach the server. Check your connection and try again.",
    };
  }
}

export async function download(path: string, filename?: string) {
  const token = localStorage.getItem("lific_token");
  const headers: Record<string, string> = {};
  if (token) headers["Authorization"] = `Bearer ${token}`;

  const res = await fetch(`${BASE}${path}`, { headers });
  if (!res.ok) {
    let error = `HTTP ${res.status}`;
    try {
      const body = await res.json();
      error = body.error || error;
    } catch {
      // Ignore parse failure and keep status-based message.
    }
    return { ok: false as const, error };
  }

  const blob = await res.blob();
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download =
    filename ||
    res.headers
      .get("content-disposition")
      ?.match(/filename="([^"]+)"/)?.[1] ||
    "download";
  document.body.appendChild(a);
  a.click();
  a.remove();
  URL.revokeObjectURL(url);
  return { ok: true as const };
}

/** Public, unauthenticated instance metadata the auth screen reads before
 *  anyone has a session. Drives whether signup is open and whether this is a
 *  brand-new instance vs one you are joining. Never includes user data, and
 *  `has_users` never implies the new account is an admin (admin is CLI-only). */
export interface InstanceInfo {
  allow_signup: boolean;
  has_users: boolean;
  /** Human name for the instance, or null (fall back to host). */
  instance_name: string | null;
  /** Short admin message to show on the auth screen, or null. */
  login_message: string | null;
  /** LIF-215: single-user mode — the web app should auto-sign-in as the admin
   *  instead of showing the login form. */
  web_auto_login: boolean;
}

export async function getInstance() {
  return request<InstanceInfo>("/instance");
}

/** Full, admin-only instance settings (LIF-210). */
export interface InstanceSettings {
  allow_signup: boolean;
  instance_name: string | null;
  signup_email_domains: string[];
  session_lifetime_days: number;
  login_message: string | null;
  web_auto_login: boolean;
  /** LIF-197: operator toggle for project-scoped authorization (epic
   *  LIF-194). Off by default — see src/authz.rs for the legacy vs
   *  enforced mode split. */
  authz_enforced: boolean;
}

export interface InstanceSettingsPatch {
  allow_signup?: boolean;
  /** "" clears (falls back to host). */
  instance_name?: string;
  signup_email_domains?: string[];
  session_lifetime_days?: number;
  /** "" clears. */
  login_message?: string;
  web_auto_login?: boolean;
  authz_enforced?: boolean;
}

export async function getInstanceSettings() {
  return request<InstanceSettings>("/instance/settings");
}

export async function updateInstanceSettings(patch: InstanceSettingsPatch) {
  return request<InstanceSettings>("/instance/settings", {
    method: "PATCH",
    body: JSON.stringify(patch),
  });
}

export async function signup(
  username: string,
  email: string,
  password: string
) {
  return request<AuthResponse>("/auth/signup", {
    method: "POST",
    body: JSON.stringify({ username, email, password }),
  });
}

export async function login(identity: string, password: string) {
  return request<AuthResponse>("/auth/login", {
    method: "POST",
    body: JSON.stringify({ identity, password }),
  });
}

/** Single-user mode (LIF-215): mint an admin session without a password when
 *  the instance has `web_auto_login` enabled. Returns 403 when it's off. */
export async function autoLogin() {
  return request<AuthResponse>("/auth/auto-login", { method: "POST" });
}

export async function logout() {
  const result = await request("/auth/logout", { method: "POST" });
  localStorage.removeItem("lific_token");
  return result;
}

export async function me() {
  return request<AuthUser>("/auth/me");
}

// LIF-190: account settings.
export async function updateProfile(input: { display_name?: string; email?: string }) {
  return request<AuthUser>("/auth/me", {
    method: "PATCH",
    body: JSON.stringify(input),
  });
}

export async function changePassword(input: { current_password: string; new_password: string }) {
  return request<{ ok: boolean }>("/auth/me/password", {
    method: "POST",
    body: JSON.stringify(input),
  });
}

/** Sign out of every session (this one too). Clears the local token. */
export async function revokeAllSessions() {
  const result = await request<{ revoked: boolean }>("/auth/me/sessions", { method: "DELETE" });
  localStorage.removeItem("lific_token");
  return result;
}

export function saveSession(token: string) {
  localStorage.setItem("lific_token", token);
}

export function clearSession() {
  localStorage.removeItem("lific_token");
}

export function hasSession(): boolean {
  return !!localStorage.getItem("lific_token");
}

// ── API Key management ──────────────────────────────────────

export interface ApiKey {
  id: number;
  name: string;
  created_at: string;
  expires_at: string | null;
  revoked: boolean;
}

export interface CreateKeyResponse {
  name: string;
  key: string;
}

export async function listKeys() {
  return request<ApiKey[]>("/auth/keys");
}

export async function createKey(name: string) {
  return request<CreateKeyResponse>("/auth/keys", {
    method: "POST",
    body: JSON.stringify({ name }),
  });
}

export async function revokeKey(id: number) {
  return request<{ revoked: boolean }>(`/auth/keys/${id}`, {
    method: "DELETE",
  });
}

// ── Bot (connected tool) management ─────────────────────────

export interface Bot {
  id: number;
  username: string;
  display_name: string;
  owner_id: number | null;
  created_at: string;
  has_active_key: boolean;
}

export interface CreateBotResponse {
  bot: { id: number; username: string; display_name: string };
  key: string;
  tool: string;
}

export async function listBots() {
  return request<Bot[]>("/auth/bots");
}

export async function createBot(tool: string) {
  return request<CreateBotResponse>("/auth/bots", {
    method: "POST",
    body: JSON.stringify({ tool }),
  });
}

export async function disconnectBot(id: number) {
  return request<{ disconnected: boolean }>(`/auth/bots/${id}/disconnect`, {
    method: "POST",
  });
}

export async function deleteBot(id: number) {
  return request<{ deleted: boolean }>(`/auth/bots/${id}`, {
    method: "DELETE",
  });
}

// ── Users ───────────────────────────────────────────────────

export interface UserSummary {
  id: number;
  username: string;
  display_name: string;
  is_admin: boolean;
  created_at: string;
}

export async function listUsers() {
  return request<UserSummary[]>("/users");
}

// ── Project members (LIF-199 / LIF-200) ─────────────────────
//
// Project-scoped roles: viewer < maintainer < lead. See LIF-DOC-7 for the
// authorization design. `GET` is visible to any project member (and to
// everyone while `authz_enforced` is off); writes are lead-only (or
// instance admin).

export type ProjectRole = "viewer" | "maintainer" | "lead";

export interface ProjectMember {
  project_id: number;
  user_id: number;
  role: ProjectRole;
  created_at: string;
  username: string;
  display_name: string;
}

export async function listProjectMembers(projectId: number) {
  return request<ProjectMember[]>(`/projects/${projectId}/members`);
}

export async function addProjectMember(
  projectId: number,
  input: { user_id: number; role?: ProjectRole },
) {
  return request<ProjectMember>(`/projects/${projectId}/members`, {
    method: "POST",
    body: JSON.stringify(input),
  });
}

export async function changeProjectMemberRole(
  projectId: number,
  userId: number,
  role: ProjectRole,
) {
  return request<ProjectMember>(`/projects/${projectId}/members/${userId}`, {
    method: "PATCH",
    body: JSON.stringify({ role }),
  });
}

export async function removeProjectMember(projectId: number, userId: number) {
  return request<{ deleted: boolean }>(`/projects/${projectId}/members/${userId}`, {
    method: "DELETE",
  });
}

// ── @mention candidates (LIF-263) ───────────────────────────
//
// The users a comment composer may `@`-mention in this project. Scoped to
// project members when `authz_enforced` is on, all non-bot users otherwise
// — the server never returns anyone who can't see the project.

export interface MentionCandidate {
  user_id: number;
  username: string;
  display_name: string;
}

export async function listMentionCandidates(projectId: number) {
  return request<MentionCandidate[]>(`/projects/${projectId}/mention-candidates`);
}

// ── Projects ────────────────────────────────────────────────

export interface Project {
  id: number;
  name: string;
  identifier: string;
  description: string;
  emoji: string | null;
  lead_user_id: number | null;
  // LIF-233: sidebar ordering rank (server-assigned, 0..N).
  sort_order: number;
  created_at: string;
  updated_at: string;
}

export async function listProjects() {
  return request<Project[]>("/projects");
}

// LIF-233: persist sidebar order. Send the full id list top-to-bottom; the
// server reindexes sort_order to match. Returns the reordered project list.
export async function reorderProjects(ids: number[]) {
  return request<Project[]>("/projects/reorder", {
    method: "PUT",
    body: JSON.stringify({ ids }),
  });
}

export async function getProject(id: number) {
  return request<Project>(`/projects/${id}`);
}

export interface CreateProjectInput {
  name: string;
  identifier: string;
  description?: string;
  emoji?: string;
  lead_user_id?: number;
}

export async function createProject(input: CreateProjectInput) {
  return request<Project>("/projects", {
    method: "POST",
    body: JSON.stringify(input),
  });
}

export interface UpdateProjectInput {
  name?: string;
  identifier?: string;
  description?: string;
  // LIF-103: nullable so clients can explicitly clear (PATCH semantics).
  // Omit key = "don't change", null = "set to NULL".
  emoji?: string | null;
  lead_user_id?: number | null;
}

export async function updateProject(id: number, input: UpdateProjectInput) {
  return request<Project>(`/projects/${id}`, {
    method: "PUT",
    body: JSON.stringify(input),
  });
}

export async function deleteProject(id: number) {
  return request<{ deleted: boolean }>(`/projects/${id}`, {
    method: "DELETE",
  });
}

export async function downloadProjectExport(identifier: string) {
  return download(`/export/projects/${identifier}`);
}

// ── Import (LIF-264) ────────────────────────────────────────

export interface GithubImportRequest {
  repo: string;
  token?: string;
  state?: "open" | "closed" | "all";
  map_open?: string;
  map_closed?: string;
  dry_run: boolean;
}

export interface ImportSummary {
  dry_run: boolean;
  issues_created: number;
  issues_skipped_existing: number;
  comments_created: number;
  labels_created: number;
  comments_planned: number;
  labels_planned: number;
  skipped_non_issues: number;
  skipped_assignees: number;
  skipped_other: number;
}

export async function importGithub(projectId: number, input: GithubImportRequest) {
  return request<ImportSummary>(`/projects/${projectId}/import/github`, {
    method: "POST",
    body: JSON.stringify(input),
  });
}

// ── Issues ──────────────────────────────────────────────────

export interface Issue {
  id: number;
  project_id: number;
  sequence: number;
  identifier: string;
  title: string;
  description: string;
  status: string;
  priority: string;
  module_id: number | null;
  sort_order: number;
  start_date: string | null;
  target_date: string | null;
  created_at: string;
  updated_at: string;
  labels: string[];
  blocks?: string[];
  blocked_by?: string[];
  relates_to?: string[];
}

export interface IssueFilters {
  project_id?: number;
  status?: string;
  priority?: string;
  module_id?: number;
  label?: string;
  workable?: boolean;
  limit?: number;
  offset?: number;
}

export async function listIssues(filters: IssueFilters) {
  const params = new URLSearchParams();
  for (const [k, v] of Object.entries(filters)) {
    if (v !== undefined && v !== null) params.set(k, String(v));
  }
  return request<Issue[]>(`/issues?${params}`);
}

export async function getIssue(id: number) {
  return request<Issue>(`/issues/${id}`);
}

/** Per-status issue counts for a project (LIF-161). Server-side GROUP BY —
 *  the list endpoint is limit-capped, so counting its rows undercounts. */
export interface IssueStatusCounts {
  backlog: number;
  todo: number;
  active: number;
  done: number;
  cancelled: number;
  total: number;
}

export async function getIssueCounts(projectId: number) {
  return request<IssueStatusCounts>(`/projects/${projectId}/issue-counts`);
}

export interface CreateIssueInput {
  project_id: number;
  title: string;
  description?: string;
  status?: string;
  priority?: string;
  module_id?: number;
  labels?: string[];
}

export async function createIssue(input: CreateIssueInput) {
  return request<Issue>("/issues", {
    method: "POST",
    body: JSON.stringify(input),
  });
}

export async function resolveIssue(identifier: string) {
  return request<Issue>(`/issues/resolve/${identifier}`);
}

export interface UpdateIssueInput {
  title?: string;
  description?: string;
  status?: string;
  priority?: string;
  module_id?: number;
  sort_order?: number;
  labels?: string[];
}

export async function updateIssue(id: number, input: UpdateIssueInput) {
  return request<Issue>(`/issues/${id}`, {
    method: "PUT",
    body: JSON.stringify(input),
  });
}

export async function deleteIssue(id: number) {
  return request<{ deleted: boolean }>(`/issues/${id}`, {
    method: "DELETE",
  });
}

export async function downloadIssueExport(identifier: string) {
  return download(`/export/issues/${identifier}`);
}

// ── Modules ─────────────────────────────────────────────────

export interface Module {
  id: number;
  project_id: number;
  name: string;
  description: string;
  status: string;
  /** Icon: "lucide:<Name>" or a literal emoji char. Null = no icon. */
  emoji: string | null;
  created_at: string;
  updated_at: string;
}

export async function listModules(projectId: number) {
  return request<Module[]>(`/modules?project_id=${projectId}`);
}

export async function getModule(id: number) {
  return request<Module>(`/modules/${id}`);
}

export interface CreateModuleInput {
  project_id: number;
  name: string;
  description?: string;
  status?: string;
  emoji?: string;
}

export async function createModule(input: CreateModuleInput) {
  return request<Module>(`/modules`, {
    method: "POST",
    body: JSON.stringify(input),
  });
}

export interface UpdateModuleInput {
  name?: string;
  description?: string;
  status?: string;
  // LIF-124: nullable so clients can clear the icon. Omit = no change,
  // null = clear, string = set.
  emoji?: string | null;
}

export async function updateModule(id: number, input: UpdateModuleInput) {
  return request<Module>(`/modules/${id}`, {
    method: "PUT",
    body: JSON.stringify(input),
  });
}

export async function deleteModule(id: number) {
  return request<{ deleted: boolean }>(`/modules/${id}`, {
    method: "DELETE",
  });
}

// ── Labels ──────────────────────────────────────────────────

export interface Label {
  id: number;
  project_id: number;
  name: string;
  color: string;
}

export async function listLabels(projectId: number) {
  return request<Label[]>(`/labels?project_id=${projectId}`);
}

export interface CreateLabelInput {
  project_id: number;
  name: string;
  /** Hex color (e.g. "#EF4444"). Server defaults to a neutral gray if omitted. */
  color?: string;
}

export async function createLabel(input: CreateLabelInput) {
  return request<Label>(`/labels`, {
    method: "POST",
    body: JSON.stringify(input),
  });
}

export interface UpdateLabelInput {
  name?: string;
  color?: string;
}

export async function updateLabel(id: number, input: UpdateLabelInput) {
  return request<Label>(`/labels/${id}`, {
    method: "PUT",
    body: JSON.stringify(input),
  });
}

export async function deleteLabel(id: number) {
  return request<{ deleted: boolean }>(`/labels/${id}`, {
    method: "DELETE",
  });
}

/** Merge label `id` into label `into`: re-points every issue/page attachment
 *  onto the target (deduped) then deletes the source. Returns the survivor. */
export async function mergeLabel(id: number, into: number) {
  return request<Label>(`/labels/${id}/merge`, {
    method: "POST",
    body: JSON.stringify({ into }),
  });
}

// ── Comments ────────────────────────────────────────────────

export interface Comment {
  id: number;
  /** Set for issue comments; null for page comments. */
  issue_id: number | null;
  /** Set for page comments; null for issue comments. */
  page_id: number | null;
  user_id: number;
  author: string;
  author_display_name: string;
  content: string;
  created_at: string;
  updated_at: string;
}

// ── Activity / audit log (LIF-156/157) ─────────────────

export interface Activity {
  id: number;
  ts: string;
  actor_user_id: number | null;
  actor_username: string | null;
  actor_display_name: string | null;
  actor_is_bot: boolean;
  /** web | mcp | api | cli | system */
  transport: string;
  entity_type: string;
  entity_id: number;
  entity_label: string | null;
  project_id: number | null;
  issue_id: number | null;
  page_id: number | null;
  /** create | update | delete | attach | detach | link | unlink */
  action: string;
  field: string | null;
  old_value: string | null;
  new_value: string | null;
}

export interface ActivityFeed {
  items: Activity[];
  has_more: boolean;
}

export async function listIssueActivity(issueId: number, limit = 100) {
  return request<ActivityFeed>(`/issues/${issueId}/activity?limit=${limit}`);
}

export async function listPageActivity(pageId: number, limit = 100) {
  return request<ActivityFeed>(`/pages/${pageId}/activity?limit=${limit}`);
}

export async function listPlanActivity(planId: number, limit = 100) {
  return request<ActivityFeed>(`/plans/${planId}/activity?limit=${limit}`);
}

export async function listProjectActivity(projectId: number, limit = 50, offset = 0) {
  return request<ActivityFeed>(
    `/projects/${projectId}/activity?limit=${limit}&offset=${offset}`,
  );
}

/** Per-actor rollup for a project's audit history (most active first). */
export interface ActorStat {
  actor_user_id: number | null;
  username: string | null;
  display_name: string | null;
  is_bot: boolean;
  actions: number;
  last_ts: string;
  top_transport: string;
}

export async function listProjectActivityActors(projectId: number) {
  return request<ActorStat[]>(`/projects/${projectId}/activity/actors`);
}

export async function listComments(issueId: number) {
  return request<Comment[]>(`/issues/${issueId}/comments`);
}

export async function createComment(issueId: number, content: string) {
  return request<Comment>(`/issues/${issueId}/comments`, {
    method: "POST",
    body: JSON.stringify({ content }),
  });
}

export async function listPageComments(pageId: number) {
  return request<Comment[]>(`/pages/${pageId}/comments`);
}

export async function createPageComment(pageId: number, content: string) {
  return request<Comment>(`/pages/${pageId}/comments`, {
    method: "POST",
    body: JSON.stringify({ content }),
  });
}

// ── Attachments (LIF-262) ────────────────────────────────────
//
// Image + file uploads on issues, comments, and pages. Bytes are stored
// content-addressed server-side; the client only ever holds the numeric id
// and the `/api/attachments/{id}` URL. Uploads go through a raw `fetch` (not
// the JSON `request` helper) because they're multipart, not JSON.

export interface Attachment {
  id: number;
  filename: string;
  mime: string;
  size_bytes: number;
  uploader_id: number | null;
  created_at: string;
}

export interface UploadResponse {
  id: number;
  url: string;
  filename: string;
  mime: string;
  size: number;
}

export type AttachmentEntity = "issue" | "page" | "comment";

/** Upload one file. Optionally link it to an entity immediately (used by the
 *  detail-view "Attach" affordance); otherwise it stays unlinked until the
 *  entity's markdown is saved and re-scanned server-side. Returns a discrete
 *  result so callers can surface the exact server reason via a toast. */
export async function uploadAttachment(
  file: File,
  link?: { entity_type: AttachmentEntity; entity_id: number },
): Promise<{ ok: true; data: UploadResponse } | { ok: false; error: string }> {
  const token = localStorage.getItem("lific_token");
  const form = new FormData();
  form.append("file", file, file.name);
  if (link) {
    form.append("entity_type", link.entity_type);
    form.append("entity_id", String(link.entity_id));
  }
  const headers: Record<string, string> = {};
  if (token) headers["Authorization"] = `Bearer ${token}`;
  try {
    const res = await fetch(`${BASE}/attachments`, {
      method: "POST",
      headers,
      body: form,
    });
    const body = await res.json();
    if (!res.ok) {
      return { ok: false, error: body.error || `HTTP ${res.status}` };
    }
    return { ok: true, data: body as UploadResponse };
  } catch {
    return {
      ok: false,
      error: "Couldn't reach the server. Check your connection and try again.",
    };
  }
}

/** List the attachments linked to one entity (detail-view section). */
export async function listEntityAttachments(
  entityType: AttachmentEntity,
  entityId: number,
) {
  return request<Attachment[]>(
    `/attachments?entity_type=${entityType}&entity_id=${entityId}`,
  );
}

/** Download an attachment via the shared `download` helper (auth header +
 *  Content-Disposition filename handling). */
export async function downloadAttachment(id: number, filename?: string) {
  return download(`/attachments/${id}`, filename);
}

export async function deleteAttachment(id: number) {
  return request<{ deleted: boolean }>(`/attachments/${id}`, {
    method: "DELETE",
  });
}

/** Human-readable byte size (e.g. "1.4 MB"). Small standalone helper so the
 *  chip renderer and the detail-section share one formatting. */
export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB"];
  let val = bytes / 1024;
  let i = 0;
  while (val >= 1024 && i < units.length - 1) {
    val /= 1024;
    i++;
  }
  return `${val.toFixed(val < 10 ? 1 : 0)} ${units[i]}`;
}

// ── Pages ───────────────────────────────────────────────────

export interface Page {
  id: number;
  project_id: number | null;
  sequence: number | null;
  identifier: string;
  folder_id: number | null;
  title: string;
  content: string;
  sort_order: number;
  /** LIF-112: lifecycle status — draft | active | complete | archived. */
  status: string;
  /** LIF-183: pinned to the top of the page list. */
  pinned: boolean;
  created_at: string;
  updated_at: string;
  /** LIF-105: project-scoped labels attached to this page. Always [] for
   *  workspace pages (project_id === null). */
  labels: string[];
}

export interface Folder {
  id: number;
  project_id: number;
  parent_id: number | null;
  name: string;
  sort_order: number;
}

export async function listPages(
  projectId: number,
  folderId?: number,
  label?: string,
  status?: string,
) {
  const params = new URLSearchParams({ project_id: String(projectId) });
  if (folderId !== undefined) params.set("folder_id", String(folderId));
  if (label) params.set("label", label);
  if (status) params.set("status", status);
  return request<Page[]>(`/pages?${params}`);
}

export async function getPage(id: number) {
  return request<Page>(`/pages/${id}`);
}

export interface CreatePageInput {
  project_id: number;
  folder_id?: number;
  title: string;
  content?: string;
  /** LIF-112: lifecycle status. Defaults to "draft" server-side. */
  status?: string;
  /** LIF-105: label names to attach. Ignored on workspace pages. */
  labels?: string[];
}

export async function createPage(input: CreatePageInput) {
  return request<Page>("/pages", {
    method: "POST",
    body: JSON.stringify(input),
  });
}

export interface UpdatePageInput {
  title?: string;
  content?: string;
  folder_id?: number | null;
  /** LIF-112: lifecycle status. Omitted = no change. */
  status?: string;
  /** LIF-183: pin/unpin. Omitted = no change. */
  pinned?: boolean;
  /** LIF-105: replace the full label set. Pass [] to clear. Omitted = no change. */
  labels?: string[];
}

export async function updatePage(id: number, input: UpdatePageInput) {
  return request<Page>(`/pages/${id}`, {
    method: "PUT",
    body: JSON.stringify(input),
  });
}

export async function deletePage(id: number) {
  return request<{ deleted: boolean }>(`/pages/${id}`, {
    method: "DELETE",
  });
}

export async function downloadPageExport(identifier: string) {
  return download(`/export/pages/${identifier}`);
}

export async function listFolders(projectId: number) {
  return request<Folder[]>(`/folders?project_id=${projectId}`);
}

export async function createFolder(input: { project_id: number; name: string; parent_id?: number }) {
  return request<Folder>("/folders", {
    method: "POST",
    body: JSON.stringify(input),
  });
}

export async function deleteFolder(id: number) {
  return request<{ deleted: boolean }>(`/folders/${id}`, {
    method: "DELETE",
  });
}

// ── Search ──────────────────────────────────────────────────

export interface SearchResult {
  result_type: string;
  id: number;
  identifier: string | null;
  title: string;
  snippet: string;
  project_id: number | null;
}

export async function search(query: string, projectId?: number) {
  const params = new URLSearchParams({ query });
  if (projectId) params.set("project_id", String(projectId));
  return request<SearchResult[]>(`/search?${params}`);
}

// ── Board ───────────────────────────────────────────────────

export async function getBoard(
  projectId: number,
  groupBy: "status" | "priority" | "module" = "status"
) {
  return request<Record<string, Issue[]>>(
    `/projects/${projectId}/board?group_by=${groupBy}`
  );
}

// ── Tool config templates ───────────────────────────────────

/** Per-OS config-file locations. When all three are identical the modal
 *  collapses them to a single line; Claude Desktop is the only tool whose
 *  paths genuinely differ per OS. Windows paths use %USERPROFILE%/%APPDATA%
 *  (the ~ shorthand isn't a Windows concept).
 *
 *  `linux` is nullable: Anthropic ships no Claude Desktop app for Linux,
 *  so that tool sets linux: null and the modal hides the Linux option
 *  entirely rather than offering a config path no app reads. */
export interface OsPaths {
  linux: string | null;
  mac: string;
  windows: string;
}

/** A single instruction. `text` renders as prose; `command`, when present,
 *  renders as a bounded, copyable code chip so the runnable bit is visually
 *  distinct from the surrounding words (instead of one gray prose blob). */
export interface NoteStep {
  text?: string;
  command?: string;
}

export interface ToolTemplate {
  id: string;
  name: string;
  description: string;
  /** Config-file location per OS. Content (generateConfig) is OS-identical. */
  configPath: OsPaths;
  /** Structured setup steps shown above the config block. */
  configNote?: NoteStep[];
  generateConfig: (url: string, key: string) => string;
  /** True when the tool reads the key from an env var rather than embedding
   *  it in the config block — the modal then surfaces the key + export line
   *  separately so it's never hidden (LIF connect-flow fix). */
  usesEnvKey?: boolean;
  /** Env var the tool expects the key in (when usesEnvKey). */
  envVar?: string;
}

const MCP_URL = window.location.origin + "/mcp";

/** Helper: same path on every OS bar the Windows home prefix. */
function home(unix: string, windows: string): OsPaths {
  return { linux: unix, mac: unix, windows };
}

export const TOOL_TEMPLATES: ToolTemplate[] = [
  {
    id: "opencode",
    name: "OpenCode",
    description: "Anomaly's open-source agentic coding CLI",
    configPath: home(
      "~/.config/opencode/opencode.json",
      "%USERPROFILE%\\.config\\opencode\\opencode.json"
    ),
    configNote: [{ text: 'Add this to the "mcp" section of your config.' }],
    generateConfig: (_url, key) =>
      JSON.stringify(
        {
          lific: {
            type: "remote",
            url: MCP_URL,
            headers: { Authorization: `Bearer ${key}` },
          },
        },
        null,
        2
      ),
  },
  {
    id: "cursor",
    name: "Cursor",
    description: "AI-first code editor by Anysphere",
    configPath: home(
      "~/.cursor/mcp.json (global) · .cursor/mcp.json (project)",
      "%USERPROFILE%\\.cursor\\mcp.json (global) · .cursor\\mcp.json (project)"
    ),
    configNote: [
      { text: 'Add this to the "mcpServers" section, then reload Cursor.' },
    ],
    generateConfig: (_url, key) =>
      JSON.stringify(
        {
          lific: {
            url: MCP_URL,
            headers: { Authorization: `Bearer ${key}` },
          },
        },
        null,
        2
      ),
  },
  {
    id: "claude-code",
    name: "Claude Code",
    description: "Anthropic's CLI coding agent",
    configPath: home("~/.claude.json (user scope)", "%USERPROFILE%\\.claude.json (user scope)"),
    configNote: [
      { text: "Easiest: run this command (it writes the config for you):" },
      {
        command: `claude mcp add --transport http --scope user lific ${MCP_URL} --header "Authorization: Bearer <key>"`,
      },
      { text: 'Or add the block below to the "mcpServers" section manually.' },
    ],
    generateConfig: (_url, key) =>
      JSON.stringify(
        {
          lific: {
            type: "http",
            url: MCP_URL,
            headers: { Authorization: `Bearer ${key}` },
          },
        },
        null,
        2
      ),
  },
  {
    id: "claude",
    name: "Claude Desktop",
    description: "Anthropic's desktop client for Claude (macOS & Windows)",
    // The one tool with genuinely OS-specific paths. No Linux entry —
    // Anthropic doesn't ship Claude Desktop for Linux, so the modal omits
    // that option (Linux users want Claude Code instead).
    configPath: {
      linux: null,
      mac: "~/Library/Application Support/Claude/claude_desktop_config.json",
      windows: "%APPDATA%\\Claude\\claude_desktop_config.json",
    },
    configNote: [
      { text: "Requires mcp-remote (installed automatically by npx)." },
      { text: 'Add the block below to the "mcpServers" section, then fully restart Claude Desktop.' },
    ],
    generateConfig: (_url, key) =>
      JSON.stringify(
        {
          lific: {
            command: "npx",
            args: ["-y", "mcp-remote", MCP_URL],
            env: { AUTHORIZATION: `Bearer ${key}` },
          },
        },
        null,
        2
      ),
  },
  {
    id: "codex",
    name: "Codex",
    description: "OpenAI's CLI coding agent",
    configPath: home("~/.codex/config.toml", "%USERPROFILE%\\.codex\\config.toml"),
    configNote: [
      { text: "Add the block below under [mcp_servers] in config.toml." },
      { text: "The key is read from the LIFIC_API_KEY env var (set it in step 3)." },
    ],
    usesEnvKey: true,
    envVar: "LIFIC_API_KEY",
    generateConfig: (_url, _key) =>
      `[mcp_servers.lific]\ntransport.type = "http"\ntransport.url = "${MCP_URL}"\ntransport.bearer_token_env_var = "LIFIC_API_KEY"`,
  },
  {
    id: "pi",
    name: "Pi",
    description: "Pi coding agent (via pi-mcp-adapter)",
    configPath: home("~/.pi/agent/mcp.json", "%USERPROFILE%\\.pi\\agent\\mcp.json"),
    configNote: [
      { text: "First install the adapter, then restart Pi:" },
      { command: "pi install npm:pi-mcp-adapter" },
      { text: 'Add the block below to the "mcpServers" section. The key is read from the LIFIC_API_KEY env var (set it in step 3).' },
    ],
    usesEnvKey: true,
    envVar: "LIFIC_API_KEY",
    generateConfig: (_url, _key) =>
      JSON.stringify(
        {
          lific: {
            url: MCP_URL,
            auth: "bearer",
            bearerTokenEnv: "LIFIC_API_KEY",
            lifecycle: "keep-alive",
          },
        },
        null,
        2
      ),
  },
  {
    id: "vscode",
    name: "VS Code",
    description: "GitHub Copilot agent mode in VS Code",
    configPath: home(
      "~/.config/Code/User/mcp.json (user) · .vscode/mcp.json (workspace)",
      "%APPDATA%\\Code\\User\\mcp.json (user) · .vscode\\mcp.json (workspace)"
    ),
    configNote: [
      { text: 'Add this to the "servers" section. Or run the command palette action:' },
      { command: "MCP: Open User Configuration" },
      { text: "VS Code 1.101+ with GitHub Copilot is required." },
    ],
    generateConfig: (_url, key) =>
      JSON.stringify(
        {
          servers: {
            lific: {
              type: "http",
              url: MCP_URL,
              headers: { Authorization: `Bearer ${key}` },
            },
          },
        },
        null,
        2
      ),
  },
  {
    id: "zed",
    name: "Zed",
    description: "High-performance Rust-based editor",
    configPath: home(
      "~/.config/zed/settings.json",
      "%APPDATA%\\Zed\\settings.json"
    ),
    configNote: [
      { text: 'Add this to the "context_servers" section of your Zed settings (Command Palette: zed: open settings).' },
    ],
    generateConfig: (_url, key) =>
      JSON.stringify(
        {
          context_servers: {
            lific: {
              url: MCP_URL,
              headers: { Authorization: `Bearer ${key}` },
            },
          },
        },
        null,
        2
      ),
  },
];

// ── Plans (LIF-173) ─────────────────────────────────────────

export interface PlanStep {
  id: number;
  plan_id: number;
  parent_step_id: number | null;
  position: number;
  title: string;
  description: string;
  issue_id: number | null;
  issue_identifier?: string;
  issue_status?: string;
  done: boolean;
  reopened_via_issue_at?: string;
  created_at: string;
  edited_at: string | null;
  children: PlanStep[];
}

export interface Plan {
  id: number;
  project_id: number;
  sequence: number;
  identifier: string;
  issue_id: number | null;
  anchor_identifier?: string;
  title: string;
  status: string;
  created_at: string;
  updated_at: string;
  steps: PlanStep[];
  step_count: number;
  done_count: number;
}

export interface StepDoneEffect {
  step_id: number;
  done: boolean;
  issue_identifier?: string;
  issue_status_changed: boolean;
  issue_new_status?: string;
}

export async function listPlans(projectId: number, status?: string) {
  const params = new URLSearchParams({ project_id: String(projectId) });
  if (status) params.set("status", status);
  return request<Plan[]>(`/plans?${params}`);
}

export async function getPlan(id: number) {
  return request<Plan>(`/plans/${id}`);
}

export async function resolvePlan(identifier: string) {
  return request<Plan>(`/plans/resolve/${identifier}`);
}

export interface CreatePlanStepInput {
  title: string;
  description?: string;
  issue_id?: number | null;
  done?: boolean;
  steps?: CreatePlanStepInput[];
}

export interface CreatePlanInput {
  project_id: number;
  title: string;
  issue_id?: number | null;
  steps?: CreatePlanStepInput[];
}

export async function createPlan(input: CreatePlanInput) {
  return request<Plan>("/plans", { method: "POST", body: JSON.stringify(input) });
}

export interface UpdatePlanInput {
  title?: string;
  status?: string;
  issue_id?: number | null;
}

export async function updatePlan(id: number, input: UpdatePlanInput) {
  return request<Plan>(`/plans/${id}`, { method: "PUT", body: JSON.stringify(input) });
}

export async function deletePlan(id: number) {
  return request<{ deleted: boolean }>(`/plans/${id}`, { method: "DELETE" });
}

export interface AddStepInput {
  parent_step_id?: number | null;
  title: string;
  description?: string;
  issue_id?: number | null;
}

export async function addPlanStep(planId: number, input: AddStepInput) {
  return request<Plan>(`/plans/${planId}/steps`, {
    method: "POST",
    body: JSON.stringify(input),
  });
}

export interface UpdateStepInput {
  title?: string;
  description?: string;
  done?: boolean;
  issue_id?: number | null;
  move_parent_step_id?: number;
  move_to_root?: boolean;
  move_position?: number;
}

export interface StepUpdateResponse {
  plan: Plan;
  effect?: StepDoneEffect;
}

export async function updatePlanStep(
  planId: number,
  stepId: number,
  input: UpdateStepInput,
) {
  return request<StepUpdateResponse>(`/plans/${planId}/steps/${stepId}`, {
    method: "PUT",
    body: JSON.stringify(input),
  });
}

export async function deletePlanStep(planId: number, stepId: number) {
  return request<Plan>(`/plans/${planId}/steps/${stepId}`, { method: "DELETE" });
}

// ── Home (LIF-237) ───────────────────────────────────────────
//
// The "My Work" landing dashboard reuses existing endpoints for almost
// everything (cross-project `listIssues`/`listPages` already filter to
// visible projects server-side when `project_id` is omitted — see
// LIF-197). The one new call is a cross-project pages fetch: `listPages`
// above always sends `project_id`, and there's no server-side `pinned`
// filter to combine with a cross-project scope, so Home fetches every
// visible page once and filters to `pinned` client-side. That's a single
// round trip regardless of project count (cheaper than one listPages()
// per project) and acceptable because `list_pages` has no row cap and
// real instances run a modest page count; revisit with a dedicated
// `?pinned=` filter if that stops holding.

export async function listAllPages() {
  return request<Page[]>("/pages");
}

// ── Insights (LIF-240) ───────────────────────────────────────
//
// Per-project analytics tab. One endpoint returns the full payload —
// trend lines, current distributions, and a windowed actor rollup — so
// the route makes a single round trip regardless of `weeks`.

export interface WeekPoint {
  /** Monday (ISO week start), formatted YYYY-MM-DD. */
  week_start: string;
  count: number;
}

export interface PriorityCounts {
  urgent: number;
  high: number;
  medium: number;
  low: number;
  none: number;
  total: number;
}

export interface ModuleCount {
  module_id: number | null;
  /** "No module" when module_id is null. */
  name: string;
  count: number;
}

export interface InsightsPayload {
  /** The clamped week count this payload was computed over. */
  weeks: number;
  created_per_week: WeekPoint[];
  /** See the backend's `queries::insights` module doc for the exact
   *  closure-counting semantics (latest status transition per issue). */
  closed_per_week: WeekPoint[];
  status_counts: IssueStatusCounts;
  priority_counts: PriorityCounts;
  module_counts: ModuleCount[];
  /** Actor rollup scoped to the same `weeks` window as the trend lines
   *  (unlike ActorStat's all-time rollup on the Activity tab). */
  top_actors: ActorStat[];
}

export async function getInsights(projectId: number, weeks: number) {
  return request<InsightsPayload>(`/projects/${projectId}/insights?weeks=${weeks}`);
}

// ── Saved views (LIF-242) ─────────────────────────────────────
//
// Named filter/group/sort/display presets per project, personal to each
// user — no team-shared views. `config` is an opaque JSON string as far as
// this client and the backend are concerned; `web/src/lib/issues/views.ts`
// owns the actual `ViewConfig` shape and (de)serializes it.

export interface SavedView {
  id: number;
  project_id: number;
  user_id: number;
  name: string;
  /** Opaque JSON string — see `views.ts`'s `parseConfig` / `buildConfig`. */
  config: string;
  is_default: boolean;
  created_at: string;
  updated_at: string;
}

export interface CreateSavedViewInput {
  name: string;
  config: string;
  is_default?: boolean;
}

export interface UpdateSavedViewInput {
  name?: string;
  config?: string;
  is_default?: boolean;
}

/** Lists only the caller's own views (server-enforced ownership). */
export async function listSavedViews(projectId: number) {
  return request<SavedView[]>(`/projects/${projectId}/views`);
}

export async function createSavedView(projectId: number, input: CreateSavedViewInput) {
  return request<SavedView>(`/projects/${projectId}/views`, {
    method: "POST",
    body: JSON.stringify(input),
  });
}

export async function updateSavedView(
  projectId: number,
  viewId: number,
  input: UpdateSavedViewInput,
) {
  return request<SavedView>(`/projects/${projectId}/views/${viewId}`, {
    method: "PATCH",
    body: JSON.stringify(input),
  });
}

export async function deleteSavedView(projectId: number, viewId: number) {
  return request<{ deleted: boolean }>(`/projects/${projectId}/views/${viewId}`, {
    method: "DELETE",
  });
}

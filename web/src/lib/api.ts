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
    return { ok: false, error: "Network error — is the server running?" };
  }
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

export async function logout() {
  const result = await request("/auth/logout", { method: "POST" });
  localStorage.removeItem("lific_token");
  return result;
}

export async function me() {
  return request<AuthUser>("/auth/me");
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

// ── Projects ────────────────────────────────────────────────

export interface Project {
  id: number;
  name: string;
  identifier: string;
  description: string;
  emoji: string | null;
  created_at: string;
  updated_at: string;
}

export async function listProjects() {
  return request<Project[]>("/projects");
}

export async function getProject(id: number) {
  return request<Project>(`/projects/${id}`);
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

// ── Modules ─────────────────────────────────────────────────

export interface Module {
  id: number;
  project_id: number;
  name: string;
  description: string;
  status: string;
  created_at: string;
  updated_at: string;
}

export async function listModules(projectId: number) {
  return request<Module[]>(`/modules?project_id=${projectId}`);
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

// ── Comments ────────────────────────────────────────────────

export interface Comment {
  id: number;
  issue_id: number;
  user_id: number;
  author: string;
  author_display_name: string;
  content: string;
  created_at: string;
  updated_at: string;
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

export interface ToolTemplate {
  id: string;
  name: string;
  description: string;
  configPath: string;
  configNote?: string;
  generateConfig: (url: string, key: string) => string;
}

const MCP_URL = window.location.origin + "/mcp";

export const TOOL_TEMPLATES: ToolTemplate[] = [
  {
    id: "opencode",
    name: "OpenCode",
    description: "Anomaly's open-source agentic coding CLI",
    configPath: "~/.config/opencode/opencode.json",
    configNote: "Add to the \"mcp\" section of your config",
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
    configPath: ".cursor/mcp.json (project) or ~/.cursor/mcp.json (global)",
    configNote: "Add to the \"mcpServers\" section",
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
    configPath: "~/.claude/mcp.json (global) or .mcp.json (project)",
    configNote:
      'Add to the "mcpServers" section. Or run: claude mcp add lific --transport http ' + MCP_URL,
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
    description: "Anthropic's desktop client for Claude",
    configPath:
      "~/.config/Claude/claude_desktop_config.json (Linux) or ~/Library/Application Support/Claude/claude_desktop_config.json (macOS)",
    configNote:
      "Requires mcp-remote (npm). Add to the \"mcpServers\" section. Restart Claude Desktop after editing.",
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
    configPath: "~/.codex/config.toml",
    configNote:
      'Add to config.toml under [mcp_servers]. Set the env var LIFIC_API_KEY to the key below.',
    generateConfig: (_url, key) =>
      `[mcp_servers.lific]\ntransport.type = "http"\ntransport.url = "${MCP_URL}"\ntransport.bearer_token_env_var = "LIFIC_API_KEY"\n\n# Set this environment variable:\n# export LIFIC_API_KEY="${key}"`,
  },
];

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

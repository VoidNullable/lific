import { describe, expect, it } from "bun:test";
// api.ts reads the page origin when it loads.
(globalThis as any).window ??= { location: { origin: "http://lific.test" } };
const { accountId, accountIdProblem, agentStatus, cleanAccountId, suggestAccountId, templateFor } = await import(
  "../src/lib/settings/agents"
);

describe("templateFor", () => {
  it("uses a tool's own template for its ID", () => {
    expect(templateFor("codex").name).toBe("Codex");
  });

  it("uses the tool's setup for a named variant, keeping the variant's ID and name", () => {
    const t = templateFor("codex-laptop", "Codex on the laptop");
    expect(t.id).toBe("codex-laptop");
    expect(t.name).toBe("Codex on the laptop");
    expect(t.usesEnvKey).toBe(templateFor("codex").usesEnvKey);
  });

  it("prefers the longest matching tool ID", () => {
    expect(templateFor("claude-code-work").description).toBe(templateFor("claude-code").description);
  });

  it("falls back to plain HTTP MCP instructions", () => {
    const t = templateFor("my-bot");
    expect(t.configNote?.[0].text).toContain("HTTP MCP settings");
  });
});

describe("account IDs", () => {
  it("reads legacy bots from their username", () => {
    expect(accountId({ tool_id: null, username: "cursor-blake" })).toBe("cursor");
    expect(accountId({ tool_id: "codex-laptop", username: "codex-laptop-blake" })).toBe("codex-laptop");
  });

  it("suggests the tool ID, then numbered ones", () => {
    expect(suggestAccountId("codex", new Set())).toBe("codex");
    expect(suggestAccountId("codex", new Set(["codex", "codex-2"]))).toBe("codex-3");
  });

  it("cleans typed input", () => {
    expect(cleanAccountId("Codex Laptop!")).toBe("codex-laptop");
  });

  it("names problems", () => {
    expect(accountIdProblem("", new Set())).toBe("Give it an ID");
    expect(accountIdProblem("-x", new Set())).toBe("Start with a letter or digit");
    expect(accountIdProblem("codex", new Set(["codex"]))).toBe("You already have an agent with this ID");
    expect(accountIdProblem("codex-2", new Set(["codex"]))).toBeNull();
  });
});

describe("agentStatus", () => {
  it("tells a key from an OAuth sign-in from nothing", () => {
    expect(agentStatus({ connected: true, key_created_at: "2026-01-01 00:00:00", oauth: false })).toBe("key");
    expect(agentStatus({ connected: true, key_created_at: null, oauth: true })).toBe("oauth");
    expect(agentStatus({ connected: false, key_created_at: null, oauth: false })).toBe("none");
  });

  it("assumes a key when an older server leaves the details out", () => {
    expect(agentStatus({ connected: true })).toBe("key");
  });
});

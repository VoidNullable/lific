import { TOOL_TEMPLATES, type Bot, type ToolTemplate } from "../api";

/** An agent account's connection ID: the `{id}` in its `{id}-{owner}`
 *  username. Same grammar the server enforces. */
export const ACCOUNT_ID_PATTERN = /^[a-z0-9][a-z0-9_-]{0,47}$/;

/** The ID a bot was created under. Bots made before `tool_id` existed carry
 *  it only in their username. */
export function accountId(bot: Pick<Bot, "tool_id" | "username">): string {
  if (bot.tool_id) return bot.tool_id;
  const known = TOOL_TEMPLATES.find((t) => bot.username.startsWith(`${t.id}-`));
  return known?.id ?? bot.username;
}

/** The setup template for an account ID. `codex-laptop` uses Codex's; an ID
 *  no tool claims gets plain HTTP MCP instructions. */
export function templateFor(id: string, name = id): ToolTemplate {
  const exact = TOOL_TEMPLATES.find((t) => t.id === id);
  if (exact) return exact;
  const prefixed = [...TOOL_TEMPLATES]
    .sort((a, b) => b.id.length - a.id.length)
    .find((t) => id.startsWith(`${t.id}-`));
  if (prefixed) return { ...prefixed, id, name };
  return genericTemplate(id, name);
}

export function genericTemplate(id: string, name: string): ToolTemplate {
  return {
    id,
    name,
    description: "Any client that speaks MCP over HTTP",
    configPath: { linux: null, mac: null, windows: null },
    configNote: [
      {
        text: "Use this URL and Authorization header in your client's HTTP MCP settings. The exact config format depends on your client.",
      },
    ],
    generateConfig: (url, key) => JSON.stringify({ url, headers: { Authorization: `Bearer ${key}` } }, null, 2),
  };
}

/** A free account ID for a tool: the tool's own ID first, then `-2`, `-3`... */
export function suggestAccountId(tool: string, taken: ReadonlySet<string>): string {
  if (!taken.has(tool)) return tool;
  for (let n = 2; n < 100; n++) {
    const candidate = `${tool}-${n}`;
    if (!taken.has(candidate)) return candidate;
  }
  return tool;
}

/** Lowercase, and only the characters an account ID allows. */
export function cleanAccountId(raw: string): string {
  return raw.toLowerCase().replace(/\s+/g, "-").replace(/[^a-z0-9_-]/g, "").slice(0, 48);
}

/** Why the server would refuse this ID, or null. */
export function accountIdProblem(id: string, taken: ReadonlySet<string>): string | null {
  if (!id) return "Give it an ID";
  if (!ACCOUNT_ID_PATTERN.test(id)) return "Start with a letter or digit";
  if (taken.has(id)) return "You already have an agent with this ID";
  return null;
}

export type AgentStatus = "key" | "oauth" | "none";

/** How the account is connected right now. */
export function agentStatus(bot: Pick<Bot, "connected" | "oauth" | "key_created_at">): AgentStatus {
  if (!bot.connected) return "none";
  if (bot.key_created_at) return "key";
  return bot.oauth ? "oauth" : "key";
}

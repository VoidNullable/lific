// opencode-lific-plans — Lific-backed planning for OpenCode.
//
// This plugin OVERRIDES the builtin `todowrite` tool. Because the OpenCode TUI
// keys its special todo rendering on the literal tool name `todowrite`
// (packages/tui/src/routes/session/index.tsx — a <Switch> on toolDisplay()),
// keeping the name means the plan renders with the exact native todo block.
// On top of that, every planning call is persisted to a Lific **plan** (one per
// OpenCode session, per project), so the plan survives across sessions and
// compaction and is editable in the Lific web UI.
//
// Multi-project: the tool takes an optional `project` arg (Lific project
// identifier). Different projects in the same session get distinct plans.
//
// Hard dependency (by design): when Lific is configured, a failed Lific write
// THROWS so planning visibly fails if Lific is down — a forcing function to
// keep Lific running. When Lific is NOT configured, the tool falls back to pure
// native behavior (render only), so it is always safe to load.
//
// Config (plugin options OR env):
//   LIFIC_URL, LIFIC_API_KEY, LIFIC_PLAN_PROJECT (default project identifier)

import type { Plugin } from "@opencode-ai/plugin";
import { tool } from "@opencode-ai/plugin";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

interface Step {
  id: number;
  title: string;
  done: boolean;
  children: Step[];
}
interface Plan {
  id: number;
  identifier: string;
  title: string;
  status: string;
  steps: Step[];
  step_count: number;
  done_count: number;
}
interface Todo {
  content: string;
  status: string;
  priority?: string;
}
interface Cfg {
  url: string;
  apiKey: string;
  project: string;
}

function loadConfig(options?: Record<string, unknown>): Cfg | null {
  const pick = (k: string, env: string) =>
    (typeof options?.[k] === "string" ? (options![k] as string) : "") || process.env[env] || "";
  const url = pick("url", "LIFIC_URL").replace(/\/+$/, "");
  const apiKey = pick("apiKey", "LIFIC_API_KEY");
  const project = pick("project", "LIFIC_PLAN_PROJECT");
  if (!url || !apiKey) return null;
  return { url, apiKey, project };
}

// ── Per-session JSON store: { plans: {project: planId}, latest: planId } ──
const CACHE_DIR = join(homedir(), ".cache", "opencode", "lific-plans");
function storePath(sessionID: string) {
  return join(CACHE_DIR, `${sessionID.replace(/[^A-Za-z0-9_-]/g, "_")}.json`);
}
type SessionStore = { plans: Record<string, number>; latest?: number };
function readStore(sessionID: string): SessionStore {
  try {
    return JSON.parse(readFileSync(storePath(sessionID), "utf8")) as SessionStore;
  } catch {
    return { plans: {} };
  }
}
function writeStore(sessionID: string, store: SessionStore) {
  try {
    mkdirSync(CACHE_DIR, { recursive: true });
    writeFileSync(storePath(sessionID), JSON.stringify(store));
  } catch {
    /* best-effort */
  }
}

const isDone = (status: string) => status === "completed" || status === "cancelled";

class Lific {
  constructor(private cfg: Cfg) {}
  private async req<T>(method: string, path: string, body?: unknown): Promise<T> {
    const res = await fetch(`${this.cfg.url}/api${path}`, {
      method,
      headers: { "content-type": "application/json", authorization: `Bearer ${this.cfg.apiKey}` },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    if (!res.ok) {
      const detail = await res.text().catch(() => "");
      throw new Error(`${method} ${path} → ${res.status} ${detail}`.trim());
    }
    return (res.status === 204 ? null : await res.json()) as T;
  }
  async projectId(identifier: string): Promise<number | null> {
    const projects = await this.req<Array<{ id: number; identifier: string }>>("GET", "/projects");
    return projects.find((p) => p.identifier === identifier)?.id ?? null;
  }
  getPlan(id: number) {
    return this.req<Plan>("GET", `/plans/${id}`);
  }
  createPlan(projectId: number, title: string) {
    return this.req<Plan>("POST", "/plans", { project_id: projectId, title });
  }
  setPlan(id: number, patch: Record<string, unknown>) {
    return this.req<Plan>("PUT", `/plans/${id}`, patch);
  }
  addStep(planId: number, title: string) {
    return this.req<Plan>("POST", `/plans/${planId}/steps`, { title });
  }
  setStep(planId: number, stepId: number, patch: Record<string, unknown>) {
    return this.req<unknown>("PUT", `/plans/${planId}/steps/${stepId}`, patch);
  }
  deleteStep(planId: number, stepId: number) {
    return this.req<unknown>("DELETE", `/plans/${planId}/steps/${stepId}`);
  }
}

/** Reconcile a plan's top-level steps with the flat todo list (match by
 *  content: toggle done, add new, delete gone). Returns the refreshed plan. */
async function syncTodos(lific: Lific, planId: number, todos: Todo[]): Promise<Plan> {
  let plan = await lific.getPlan(planId);
  const byTitle = new Map<string, Step>();
  for (const s of plan.steps) if (!byTitle.has(s.title)) byTitle.set(s.title, s);

  const desired = todos.map((t) => ({ title: t.content, done: isDone(t.status) }));
  const desiredTitles = new Set(desired.map((d) => d.title));

  for (const d of desired) {
    const existing = byTitle.get(d.title);
    if (existing) {
      if (existing.done !== d.done) await lific.setStep(planId, existing.id, { done: d.done });
    } else {
      const after = await lific.addStep(planId, d.title);
      const created = after.steps.find((s) => s.title === d.title);
      if (created && d.done) await lific.setStep(planId, created.id, { done: true });
    }
  }
  for (const s of plan.steps) {
    if (!desiredTitles.has(s.title)) await lific.deleteStep(planId, s.id);
  }

  const allDone = desired.length > 0 && desired.every((d) => d.done);
  plan = await lific.getPlan(planId);
  const target = allDone ? "done" : "active";
  if (plan.status !== target && plan.status !== "archived") plan = await lific.setPlan(planId, { status: target });
  return plan;
}

function renderPlanMarkdown(plan: Plan): string {
  const lines: string[] = [];
  const walk = (steps: Step[], depth: number) => {
    for (const s of steps) {
      lines.push(`${"  ".repeat(depth)}- [${s.done ? "x" : " "}] ${s.title}`);
      if (s.children?.length) walk(s.children, depth + 1);
    }
  };
  walk(plan.steps, 0);
  return lines.join("\n");
}

const TODOWRITE_DESCRIPTION = `Use this tool to create and manage a structured task list for the current coding session, and to persist it as a durable Lific plan.

Keep it updated as you work: mark exactly one task in_progress at a time, mark tasks completed the moment they are done, and add follow-ups as they appear. Use it for any non-trivial, multi-step work (3+ steps); skip it for single trivial actions.

Each todo: { content, status (pending|in_progress|completed|cancelled), priority (high|medium|low) }. The whole list is replaced on every call.

Optionally pass \`project\` (a Lific project identifier, e.g. LIF) to choose which Lific project the plan is stored under; otherwise the configured default is used. The list renders inline exactly like the native todo list AND is mirrored to a Lific plan that survives across sessions and compaction.`;

export const LificPlans: Plugin = async ({ client, worktree, directory }, options) => {
  const cfg = loadConfig(options);
  const lific = cfg ? new Lific(cfg) : null;
  const projectIdCache = new Map<string, number | null>();

  const log = (level: string, message: string) =>
    client.app.log({ body: { service: "lific-plans", level: level as never, message } }).catch(() => {});

  async function resolveProjectId(identifier: string): Promise<number | null> {
    if (projectIdCache.has(identifier)) return projectIdCache.get(identifier)!;
    const id = await lific!.projectId(identifier);
    projectIdCache.set(identifier, id);
    return id;
  }

  async function ensurePlan(sessionID: string, project: string): Promise<number> {
    const store = readStore(sessionID);
    const cached = store.plans[project];
    if (cached != null) {
      try {
        await lific!.getPlan(cached);
        return cached;
      } catch {
        /* stale — recreate */
      }
    }
    const pid = await resolveProjectId(project);
    if (pid == null) throw new Error(`project '${project}' not found in Lific`);
    const repo = (worktree || directory || "").split("/").filter(Boolean).pop() || "session";
    const short = sessionID.slice(-6);
    const plan = await lific!.createPlan(pid, `OpenCode · ${repo} · ${short}`);
    store.plans[project] = plan.id;
    store.latest = plan.id;
    writeStore(sessionID, store);
    await log("info", `created plan ${plan.identifier} (project ${project}, session ${sessionID})`);
    return plan.id;
  }

  return {
    // Override the builtin todowrite so the native renderer still fires (it is
    // name-gated to "todowrite") while we persist to Lific.
    tool: {
      todowrite: tool({
        description: TODOWRITE_DESCRIPTION,
        args: {
          todos: tool.schema
            .array(
              tool.schema.object({
                content: tool.schema.string().describe("Brief description of the task"),
                status: tool.schema
                  .string()
                  .describe("Current status: pending, in_progress, completed, cancelled"),
                priority: tool.schema.string().describe("Priority: high, medium, low").optional(),
              }),
            )
            .describe("The updated todo list"),
          project: tool.schema
            .string()
            .describe("Lific project identifier to store this plan under (e.g. LIF). Defaults to the configured project.")
            .optional(),
        },
        async execute(args, context) {
          const todos = (args.todos ?? []) as Todo[];
          const incomplete = todos.filter((t) => t.status !== "completed").length;

          // Reproduce the native todo block: the TUI reads metadata.todos.
          context.metadata({ title: `${incomplete} todos`, metadata: { todos } });

          let footer = "";
          if (lific && cfg) {
            const project = (args.project || cfg.project || "").trim();
            if (!project) {
              footer = "\n\n(Lific: no project set — pass `project` or set LIFIC_PLAN_PROJECT to persist)";
            } else {
              try {
                const planId = await ensurePlan(context.sessionID, project);
                const plan = await syncTodos(lific, planId, todos);
                const store = readStore(context.sessionID);
                store.latest = planId;
                writeStore(context.sessionID, store);
                footer = `\n\nLific plan: ${plan.identifier} — ${plan.done_count}/${plan.step_count} done`;
              } catch (err) {
                // Hard fail: planning requires Lific when it's configured.
                throw new Error(
                  `Lific planning failed — is Lific reachable at ${cfg.url}? (${String(err)})`,
                );
              }
            }
          }

          return JSON.stringify(todos, null, 2) + footer;
        },
      }),
    },

    "experimental.session.compacting": async ({ sessionID }, output) => {
      if (!lific) return;
      const planId = readStore(sessionID).latest;
      if (planId == null) return;
      try {
        const plan = await lific.getPlan(planId);
        if (plan.step_count === 0) return;
        output.context.push(
          `## Active Lific plan (${plan.identifier})\n` +
            `This session's plan lives in Lific and survives compaction. Resume from it; keep planning via the todo tool (mirrored to this plan).\n\n` +
            renderPlanMarkdown(plan),
        );
      } catch {
        /* never block compaction */
      }
    },
  };
};

export default LificPlans;

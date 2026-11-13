#!/usr/bin/env bun
/**
 * LIF-502, LIF-567: tick task-list checkboxes in an issue description and a
 * page body, straight in the live editor.
 *
 * Spawns the debug binary against a scratch config + DB, signs in through
 * /login as the instance admin and as a project viewer, and checks that:
 *
 *   - the Nth checkbox rewrites exactly the Nth real task marker, skipping
 *     `- [ ]` inside a fenced code block, saves at once and survives a reload;
 *   - ⌘/Ctrl+Enter on the caret's line toggles it back;
 *   - two quick clicks on different items both land (saves are serialized);
 *   - a body changed elsewhere is merged, not overwritten (`expected_seq`):
 *     a different part merges and saves, the same part asks first;
 *   - checkboxes in comments stay disabled;
 *   - a viewer sees disabled checkboxes and cannot change anything.
 *
 * Run:  cd e2e && bun task-checkboxes.ts
 * Binary: $LIFIC_BIN, else target/debug/lific (debug rust-embed reads
 * web/dist from disk, so build the web UI first).
 */
import { strict as assert } from "node:assert";
import { chromium, type Browser, type BrowserContext, type Page } from "playwright";
import { spawn, execFileSync, type ChildProcess } from "node:child_process";
import { mkdtempSync, rmSync, existsSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const ROOT = resolve(import.meta.dir, "..");
const BIN = process.env.LIFIC_BIN ?? join(ROOT, "target", "debug", "lific");
const ADMIN_PASSWORD = "task-admin-password-123";
const VIEWER_PASSWORD = "task-viewer-password-123";

const BODY = [
  "Pick the options you want:",
  "",
  "```md",
  "- [ ] not a task",
  "```",
  "",
  "- [ ] Option A",
  "- [ ] Option B",
  "- [ ] Option C",
  "",
].join("\n");

function cli(config: string, db: string, args: string[]): string {
  return execFileSync(BIN, ["--config", config, "--db", db, ...args], {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
  });
}

function freePort(): Promise<number> {
  return new Promise((res, rej) => {
    const srv = createServer();
    srv.listen(0, "127.0.0.1", () => {
      const addr = srv.address();
      if (addr && typeof addr === "object") {
        const port = addr.port;
        srv.close(() => res(port));
      } else srv.close(() => rej(new Error("could not allocate a port")));
    });
  });
}

async function waitForServer(url: string, timeoutMs: number): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      if ((await fetch(url)).ok) return;
    } catch {
      // not up yet
    }
    await new Promise((r) => setTimeout(r, 150));
  }
  throw new Error(`server did not answer at ${url} within ${timeoutMs}ms`);
}

/** Calls the REST API from inside the page with the session's token. */
async function api(page: Page, method: string, path: string, body?: unknown): Promise<any> {
  return page.evaluate(
    async ([method, path, body]) => {
      const res = await fetch(`/api${path}`, {
        method,
        headers: {
          Authorization: `Bearer ${localStorage.getItem("lific_token")}`,
          "Content-Type": "application/json",
        },
        body: body === undefined ? undefined : JSON.stringify(body),
      });
      if (!res.ok) throw new Error(`${method} ${path}: ${res.status} ${await res.text()}`);
      return res.json();
    },
    [method, path, body] as const,
  );
}

async function signIn(context: BrowserContext, base: string, identity: string, password: string) {
  const page = await context.newPage();
  await page.goto(`${base}/login`, { waitUntil: "load" });
  await page.fill("#login-identity", identity);
  await page.fill("#login-password", password);
  await page.click("button[type=submit]");
  await page.waitForURL((url) => !url.href.includes("login"), { timeout: 15_000 });
  // The route changes before the session token is stored; navigating away
  // in that gap loads the next page signed out.
  await page.waitForFunction(() => localStorage.getItem("lific_token"), null, { timeout: 15_000 });
  return page;
}

/** The live editor's task checkboxes. */
function editorTasks(page: Page) {
  return page.locator(".cm-content input.cm-md-task");
}

/** The read-only body's task checkboxes (not the comment thread's). */
function readTasks(page: Page) {
  return page.locator(".em-rendered .prose li input[type=checkbox]");
}

type Tasks = (page: Page) => ReturnType<Page["locator"]>;

async function checkedStates(page: Page, tasks: Tasks): Promise<boolean[]> {
  return tasks(page).evaluateAll((els) => els.map((el) => (el as HTMLInputElement).checked));
}

/** Waits for the body's task checkboxes to show `expected`. */
async function expectStates(page: Page, expected: boolean[], what: string, tasks: Tasks = editorTasks) {
  const deadline = Date.now() + 8_000;
  let last: boolean[] = [];
  while (Date.now() < deadline) {
    last = await checkedStates(page, tasks);
    if (JSON.stringify(last) === JSON.stringify(expected)) return;
    await page.waitForTimeout(100);
  }
  assert.deepEqual(last, expected, what);
}

function withTicked(body: string, ...labels: string[]): string {
  let out = body;
  for (const label of labels) out = out.replace(`- [ ] ${label}`, `- [x] ${label}`);
  return out;
}

interface Entity {
  kind: "issue" | "page";
  url: string;
  read: () => Promise<{ body: string; seq: number }>;
  write: (body: string) => Promise<void>;
}

/** Waits for the next PUT the page sends and returns its response. */
function nextPut(page: Page) {
  return page.waitForResponse((r) => r.request().method() === "PUT");
}

async function exerciseEditor(page: Page, entity: Entity) {
  const label = entity.kind;
  await page.goto(entity.url);
  await editorTasks(page).nth(2).waitFor();
  assert.equal(await editorTasks(page).count(), 3, `${label}: three task checkboxes render, none in the code block`);
  await page.locator(".cm-content", { hasText: "- [ ] not a task" }).waitFor();

  // Tick the second item: it saves at once, and the stored body changes
  // only there.
  const before = await entity.read();
  const put = nextPut(page);
  const clickedAt = Date.now();
  await editorTasks(page).nth(1).click();
  const response = await put;
  assert.ok(Date.now() - clickedAt < 1_200, `${label}: a tick saves without waiting for a typing pause`);
  assert.equal(response.status(), 200, `${label}: the toggle saves`);
  const sent = response.request().postDataJSON();
  assert.equal(sent.expected_seq, before.seq, `${label}: the save carries expected_seq`);
  const field = entity.kind === "issue" ? "description" : "content";
  assert.deepEqual(Object.keys(sent).sort(), [field, "expected_seq"].sort(), `${label}: only the body is sent`);
  await expectStates(page, [false, true, false], `${label}: second item ticked`);
  const ticked = await entity.read();
  assert.equal(ticked.body, withTicked(BODY, "Option B"), `${label}: only Option B changed`);
  assert.ok(ticked.seq > before.seq, `${label}: seq advanced`);

  await page.reload();
  await editorTasks(page).nth(2).waitFor();
  await expectStates(page, [false, true, false], `${label}: tick survives a reload`);
  console.log(`ok   ${label}: second checkbox ticks Option B only, saves at once and survives reload`);

  // Untick from the keyboard: caret on the line, ⌘/Ctrl+Enter.
  await page.locator(".cm-line", { hasText: "Option B" }).click();
  const unput = nextPut(page);
  await page.keyboard.press("ControlOrMeta+Enter");
  await unput;
  await page.keyboard.press("Escape");
  await expectStates(page, [false, false, false], `${label}: Ctrl+Enter unticks`);
  assert.equal((await entity.read()).body, BODY, `${label}: body is back to the original`);
  console.log(`ok   ${label}: Ctrl+Enter on the caret's line unticks it`);

  // Two quick clicks on different items: both must land, the second built
  // on the first save's body and seq. Each save is held for 400 ms so the
  // second click certainly lands while the first is still in flight.
  const events: string[] = [];
  const statuses: number[] = [];
  const hold = async (route: any) => {
    if (route.request().method() !== "PUT") return route.continue();
    events.push("sent");
    const response = await route.fetch();
    await new Promise((r) => setTimeout(r, 400));
    statuses.push(response.status());
    events.push("answered");
    await route.fulfill({ response });
  };
  const target = /\/api\/(issues|pages)\/\d+$/;
  await page.route(target, hold);
  await editorTasks(page).nth(0).click();
  await editorTasks(page).nth(2).click();
  const deadline = Date.now() + 8_000;
  while (statuses.length < 2 && Date.now() < deadline) await page.waitForTimeout(50);
  await expectStates(page, [true, false, true], `${label}: both quick clicks show`);
  await page.unroute(target, hold);
  assert.deepEqual(statuses, [200, 200], `${label}: both serialized saves succeed`);
  assert.deepEqual(events, ["sent", "answered", "sent", "answered"], `${label}: saves run one at a time`);
  assert.equal(
    (await entity.read()).body,
    withTicked(BODY, "Option A", "Option C"),
    `${label}: both quick clicks are stored`,
  );
  console.log(`ok   ${label}: two quick clicks both save, one after the other`);

  // Someone else edits the body while this view holds the old seq. A live
  // view would normally take the realtime event first, so this tab gets no
  // socket: it stays stale, the way a view does when an event is late or
  // the socket is down. The save is refused, the edits touch different
  // lines, so they merge and the merged text saves.
  const stale = await page.context().newPage();
  stale.setDefaultTimeout(10_000);
  await stale.routeWebSocket(/\/api\/events/, () => {});
  await stale.goto(entity.url);
  await editorTasks(stale).nth(2).waitFor();
  await expectStates(stale, [true, false, true], `${label}: stale tab loads`);
  const theirs = withTicked(BODY, "Option A", "Option C") + "\nAdded elsewhere.\n";
  await entity.write(theirs);
  const statusesStale: number[] = [];
  stale.on("response", (r) => {
    if (r.request().method() === "PUT") statusesStale.push(r.status());
  });
  await editorTasks(stale).nth(1).click();
  await stale.locator(".cm-content", { hasText: "Added elsewhere." }).waitFor();
  const merged = withTicked(theirs, "Option B");
  const mergeDeadline = Date.now() + 8_000;
  while ((await entity.read()).body !== merged && Date.now() < mergeDeadline) await stale.waitForTimeout(100);
  assert.deepEqual(statusesStale, [409, 200], `${label}: the stale save is refused, the merged one lands`);
  assert.equal((await entity.read()).body, merged, `${label}: both edits are kept`);
  await expectStates(stale, [true, true, true], `${label}: stale tab shows the merged body`);
  console.log(`ok   ${label}: a stale tick merges with an edit made elsewhere instead of overwriting it`);

  // An edit elsewhere to the same marker: nothing is saved until the person
  // picks a version.
  const sameSpot = merged.replace("- [x] Option B", "- [X] Option B");
  await entity.write(sameSpot);
  statusesStale.length = 0;
  await editorTasks(stale).nth(1).click();
  await stale.locator(".em-conflict").waitFor();
  assert.deepEqual(statusesStale, [409], `${label}: only the refused save was sent`);
  assert.equal((await entity.read()).body, sameSpot, `${label}: the other edit stands while in conflict`);
  await stale.getByRole("button", { name: "Keep mine" }).click();
  await stale.locator(".em-conflict").waitFor({ state: "detached" });
  const mine = merged.replace("- [x] Option B", "- [ ] Option B");
  const keepDeadline = Date.now() + 8_000;
  while ((await entity.read()).body !== mine && Date.now() < keepDeadline) await stale.waitForTimeout(100);
  assert.equal((await entity.read()).body, mine, `${label}: "Keep mine" saves this tab's text`);
  console.log(`ok   ${label}: an edit to the same spot asks first, then "Keep mine" saves`);

  await stale.close();
  await entity.write(BODY);
}

async function exerciseViewer(page: Page, entity: Entity) {
  const label = `viewer ${entity.kind}`;
  await page.goto(entity.url);
  await readTasks(page).nth(2).waitFor();
  assert.equal(await page.locator(".cm-content").count(), 0, `${label}: no editor`);
  assert.equal(await page.locator(".prose input.task-toggle").count(), 0, `${label}: no interactive checkboxes`);
  for (const box of await readTasks(page).all()) {
    assert.equal(await box.isDisabled(), true, `${label}: checkboxes are disabled`);
  }
  const before = await entity.read();
  let writes = 0;
  page.on("request", (r) => {
    if (r.method() === "PUT") writes += 1;
  });
  await readTasks(page).nth(1).click({ force: true });
  await page.waitForTimeout(400);
  await expectStates(page, [false, false, false], `${label}: click does nothing`, readTasks);
  assert.equal(writes, 0, `${label}: no save was attempted`);
  assert.equal((await entity.read()).body, before.body);
  console.log(`ok   ${label}: checkboxes are disabled and nothing is saved`);
}

async function main(): Promise<number> {
  if (!existsSync(BIN)) {
    console.error(`no binary at ${BIN}: run \`mkdir -p web/dist && cargo build\` (or set LIFIC_BIN)`);
    return 1;
  }
  if (!existsSync(join(ROOT, "web", "dist", "index.html"))) {
    console.error("web/dist/index.html missing: run `cd web && bun run build` first");
    return 1;
  }

  const scratch = mkdtempSync(join(tmpdir(), "lific-tasks-"));
  const config = join(scratch, "lific.toml");
  const db = join(scratch, "tasks.db");
  let server: ChildProcess | null = null;
  let browser: Browser | null = null;
  let serverLog = "";
  const deadline = setTimeout(() => {
    console.error("task checkbox test deadline exceeded");
    server?.kill("SIGKILL");
    process.exit(1);
  }, 180_000);

  try {
    cli(config, db, [
      "init", "--no-service", "--json",
      "--name", "Task Admin",
      "--auth-mode", "passwords",
      "--password", ADMIN_PASSWORD,
    ]);
    cli(config, db, ["project", "create", "--name", "Tasks", "--identifier", "TASK", "--json"]);
    cli(config, db, [
      "user", "create", "--username", "task-viewer",
      "--email", "viewer@example.test", "--password", VIEWER_PASSWORD,
    ]);
    cli(config, db, ["member", "add", "--project", "TASK", "--user", "task-viewer", "--role", "viewer"]);

    const port = await freePort();
    const base = `http://127.0.0.1:${port}`;
    server = spawn(BIN, ["--config", config, "--db", db, "start", "--port", String(port), "--host", "127.0.0.1"], {
      stdio: ["ignore", "pipe", "pipe"],
    });
    server.stdout?.on("data", (d: Buffer) => (serverLog += d.toString()));
    server.stderr?.on("data", (d: Buffer) => (serverLog += d.toString()));
    await waitForServer(`${base}/`, 30_000);

    browser = await chromium.launch({ executablePath: process.env.PLAYWRIGHT_EXECUTABLE_PATH });
    const errors: string[] = [];

    // ---- editor --------------------------------------------------------
    const adminContext = await browser.newContext({ reducedMotion: "reduce" });
    const admin = await signIn(adminContext, base, "task-admin", ADMIN_PASSWORD);
    admin.setDefaultTimeout(10_000);
    admin.on("pageerror", (err) => errors.push(String(err)));

    const projects = await api(admin, "GET", "/projects");
    const project = projects.find((p: { identifier: string }) => p.identifier === "TASK");
    const issue = await api(admin, "POST", "/issues", {
      project_id: project.id,
      title: "Choose options",
      description: BODY,
    });
    await api(admin, "POST", `/issues/${issue.id}/comments`, { content: "- [ ] a task in a comment" });
    const doc = await api(admin, "POST", "/pages", {
      project_id: project.id,
      title: "Option page",
      content: BODY,
    });

    const issueEntity = (page: Page): Entity => ({
      kind: "issue",
      url: `${base}/TASK/issues/${issue.identifier}`,
      read: async () => {
        const row = await api(page, "GET", `/issues/${issue.id}`);
        return { body: row.description, seq: row.seq };
      },
      write: async (body) => void (await api(admin, "PUT", `/issues/${issue.id}`, { description: body })),
    });
    const pageEntity = (page: Page): Entity => ({
      kind: "page",
      url: `${base}/TASK/pages/${doc.id}`,
      read: async () => {
        const row = await api(page, "GET", `/pages/${doc.id}`);
        return { body: row.content, seq: row.seq };
      },
      write: async (body) => void (await api(admin, "PUT", `/pages/${doc.id}`, { content: body })),
    });

    // Writes made by `entity.write` go through a second tab so they are not
    // mistaken for the view's own saves.
    const writer = await adminContext.newPage();
    await writer.goto(`${base}/`);
    const adminIssue = { ...issueEntity(admin), write: async (b: string) => void (await api(writer, "PUT", `/issues/${issue.id}`, { description: b })) };
    const adminPage = { ...pageEntity(admin), write: async (b: string) => void (await api(writer, "PUT", `/pages/${doc.id}`, { content: b })) };

    await exerciseEditor(admin, adminIssue);

    // A comment's checkbox stays inert even for an editor.
    await admin.goto(adminIssue.url);
    const commentBox = admin.locator(".prose", { hasText: "a task in a comment" }).locator("input[type=checkbox]");
    await commentBox.waitFor();
    assert.equal(await commentBox.isDisabled(), true, "comment checkboxes stay disabled");
    console.log("ok   issue: comment checkboxes stay disabled");

    await exerciseEditor(admin, adminPage);

    // ---- viewer --------------------------------------------------------
    const viewerContext = await browser.newContext({ reducedMotion: "reduce" });
    const viewer = await signIn(viewerContext, base, "task-viewer", VIEWER_PASSWORD);
    viewer.setDefaultTimeout(10_000);
    viewer.on("pageerror", (err) => errors.push(String(err)));
    await exerciseViewer(viewer, issueEntity(viewer));
    await exerciseViewer(viewer, pageEntity(viewer));

    assert.deepEqual(errors, [], "no uncaught page errors");
    console.log("\ntask checkbox test passed");
    return 0;
  } catch (e) {
    console.error(`FAIL ${e instanceof Error ? (e.stack ?? e.message) : e}`);
    if (serverLog.trim()) {
      console.error("\nlast server output:");
      console.error(serverLog.split("\n").slice(-25).join("\n"));
    }
    return 1;
  } finally {
    clearTimeout(deadline);
    if (browser) await browser.close().catch(() => {});
    if (server && !server.killed) {
      server.kill("SIGTERM");
      await new Promise((r) => setTimeout(r, 500));
      if (server.exitCode === null) server.kill("SIGKILL");
    }
    rmSync(scratch, { recursive: true, force: true });
  }
}

process.exit(await main());

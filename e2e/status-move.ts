#!/usr/bin/env bun
/**
 * GitHub #98: an issue whose status changes from the list or the board must
 * end up, and stay, under its new status.
 *
 * Spawns the debug binary against a scratch config + DB, signs in, and for
 * each of "realtime connected" and "realtime blocked" (a proxy that drops
 * WebSocket upgrades) checks that:
 *
 *   - the list's row status picker moves a row into the target status group;
 *   - dragging a board card to another column leaves it in that column;
 *   - neither snaps back once the write has been acknowledged.
 *
 * Run:  cd e2e && bun status-move.ts
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
const ADMIN_PASSWORD = "status-move-admin-password-123";
const PASSWORD = "status-move-user-password-123";
/** Long enough for a stale re-seed or a debounced realtime pull to land. */
const SETTLE_MS = 2_500;

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

async function api(page: Page, method: string, path: string, body?: unknown): Promise<any> {
  return page.evaluate(
    async ([method, path, body]) => {
      const res = await fetch(`/api${path}`, {
        method,
        headers: {
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
  return page;
}

async function expectStoredStatus(page: Page, id: number, status: string, what: string) {
  const deadline = Date.now() + 8_000;
  let last = "";
  while (Date.now() < deadline) {
    last = (await api(page, "GET", `/issues/${id}`)).status;
    if (last === status) return;
    await page.waitForTimeout(100);
  }
  assert.equal(last, status, what);
}

/** The status group a list row renders under, or null when it is not shown. */
function rowGroup(page: Page, identifier: string): Promise<string | null> {
  return page.evaluate((identifier) => {
    const row = [...document.querySelectorAll('[role="group"][aria-label]')].find(
      (el) => el.getAttribute("aria-label") === identifier,
    );
    return row?.closest("[data-group-key]")?.getAttribute("data-group-key") ?? null;
  }, identifier);
}

/** The board column a card renders in, or null when it is not shown. */
function cardColumn(page: Page, identifier: string): Promise<string | null> {
  return page.evaluate((identifier) => {
    const card = [...document.querySelectorAll("[data-board-cell] article")].find((el) =>
      el.textContent?.includes(identifier),
    );
    const cell = card?.closest("[data-board-cell]")?.getAttribute("data-board-cell") ?? null;
    return cell ? cell.split("::")[1] : null;
  }, identifier);
}

/** Waits for `read` to report `expected`, then requires it to hold for
 *  SETTLE_MS, so a row that lands and then snaps back still fails. */
async function expectPlacement(
  page: Page,
  read: () => Promise<string | null>,
  expected: string,
  what: string,
) {
  const deadline = Date.now() + 8_000;
  let last: string | null = null;
  while (Date.now() < deadline) {
    last = await read();
    if (last === expected) break;
    await page.waitForTimeout(100);
  }
  assert.equal(last, expected, `${what}: never shown under ${expected}`);
  const holdUntil = Date.now() + SETTLE_MS;
  while (Date.now() < holdUntil) {
    last = await read();
    assert.equal(last, expected, `${what}: snapped back to ${last}`);
    await page.waitForTimeout(100);
  }
}

async function dragCard(page: Page, identifier: string, toStatus: string) {
  const card = page.locator("[data-board-cell] article", { hasText: identifier }).first();
  const target = page.locator(`[data-board-cell$="::${toStatus}"]`).first();
  const from = await card.boundingBox();
  const to = await target.boundingBox();
  assert.ok(from && to, "card and target column are on screen");
  await page.mouse.move(from.x + from.width / 2, from.y + from.height / 2);
  await page.mouse.down();
  await page.mouse.move(from.x + from.width / 2 + 10, from.y + from.height / 2 + 10, { steps: 5 });
  await page.mouse.move(to.x + to.width / 2, to.y + 30, { steps: 20 });
  await page.waitForTimeout(150);
  await page.mouse.up();
}

async function scenario(browser: Browser, base: string, projectId: number, realtime: boolean, errors: string[]) {
  const label = realtime ? "realtime connected" : "realtime blocked";
  const context = await browser.newContext({ viewport: { width: 1440, height: 900 } });
  if (!realtime) {
    // A reverse proxy that does not forward WebSocket upgrades.
    await context.routeWebSocket(/\/api\/events\/ws/, (ws) => ws.close());
  }
  const page = await signIn(context, base, "mia", PASSWORD);
  page.setDefaultTimeout(10_000);
  page.on("pageerror", (err) => errors.push(`${label}: ${err}`));

  const create = (title: string) =>
    api(page, "POST", "/issues", { project_id: projectId, title, status: "todo" });
  const listed = await create(`List move (${label})`);
  const carded = await create(`Board move (${label})`);

  // List: the row picker.
  await page.goto(`${base}/MOVE/issues`);
  const row = page.getByRole("group", { name: listed.identifier, exact: true });
  await row.waitFor({ state: "visible", timeout: 15_000 });
  assert.equal(await rowGroup(page, listed.identifier), "todo", "row starts under todo");
  await row.getByRole("button", { name: "Status: todo" }).click();
  await row.getByRole("button", { name: "active", exact: true }).click();
  await expectStoredStatus(page, listed.id, "active", "row picker stored the status");
  await expectPlacement(page, () => rowGroup(page, listed.identifier), "active", `${label}: list row`);
  console.log(`ok   ${label}: the list row picker moves a row and it stays`);

  // Board: drag a card to another column.
  await page.goto(`${base}/MOVE/board`);
  await page.locator("[data-board-cell] article", { hasText: carded.identifier }).first().waitFor();
  assert.equal(await cardColumn(page, carded.identifier), "todo", "card starts in todo");
  await dragCard(page, carded.identifier, "done");
  await expectStoredStatus(page, carded.id, "done", "board drop stored the status");
  await expectPlacement(page, () => cardColumn(page, carded.identifier), "done", `${label}: board card`);
  console.log(`ok   ${label}: a dragged board card lands and stays`);

  await context.close();
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

  const scratch = mkdtempSync(join(tmpdir(), "lific-status-move-"));
  const config = join(scratch, "lific.toml");
  const db = join(scratch, "move.db");
  let server: ChildProcess | null = null;
  let browser: Browser | null = null;
  let serverLog = "";
  const deadline = setTimeout(() => {
    console.error("status move test deadline exceeded");
    server?.kill("SIGKILL");
    process.exit(1);
  }, 180_000);

  try {
    cli(config, db, [
      "init", "--no-service", "--json",
      "--name", "Move Admin",
      "--auth-mode", "passwords",
      "--password", ADMIN_PASSWORD,
    ]);
    cli(config, db, ["project", "create", "--name", "Move", "--identifier", "MOVE", "--json"]);
    cli(config, db, [
      "user", "create", "--username", "mia",
      "--email", "mia@example.test", "--password", PASSWORD,
    ]);
    cli(config, db, ["member", "add", "--project", "MOVE", "--user", "mia", "--role", "maintainer"]);

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

    const probe = await browser.newContext();
    const probePage = await signIn(probe, base, "mia", PASSWORD);
    const projects = await api(probePage, "GET", "/projects");
    const project = projects.find((p: { identifier: string }) => p.identifier === "MOVE");
    await probe.close();

    await scenario(browser, base, project.id, true, errors);
    await scenario(browser, base, project.id, false, errors);

    assert.deepEqual(errors, [], "no page errors");
    console.log("status move: all checks passed");
    return 0;
  } catch (err) {
    console.error(err);
    if (serverLog) console.error("--- server log ---\n" + serverLog.slice(-4000));
    return 1;
  } finally {
    clearTimeout(deadline);
    await browser?.close().catch(() => {});
    server?.kill("SIGTERM");
    rmSync(scratch, { recursive: true, force: true });
  }
}

process.exit(await main());

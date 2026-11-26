#!/usr/bin/env bun
/**
 * LIF-525: Home and the issue view show who holds a claimed issue.
 *
 * Spawns the debug binary against a scratch config + DB, signs in as two
 * project maintainers, lets one of them claim an issue over REST as a
 * worker (the `X-Lific-Worker` header an agent's session sends), and checks
 * from the other's browser that:
 *
 *   - Home lists the issue under "Claimed now" with its holder, and the
 *     agent queue leaves it out while an unclaimed issue stays in;
 *   - the issue view's "Claimed by" field names the holder and the worker;
 *   - a second worker's claim is refused with 409 naming the holder;
 *   - releasing the claim clears the field and the Home list live, from
 *     the realtime event alone (a release writes no issue row);
 *   - the Home list fits a 390px screen.
 *
 * Run:  cd e2e && bun issue-claims.ts
 * Binary: $LIFIC_BIN, else target/debug/lific (debug rust-embed reads
 * web/dist from disk, so build the web UI first).
 */
import { strict as assert } from "node:assert";
import { createHash } from "node:crypto";
import { chromium, type Browser, type BrowserContext, type Page } from "playwright";
import { spawn, execFileSync, type ChildProcess } from "node:child_process";
import { mkdtempSync, rmSync, existsSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const ROOT = resolve(import.meta.dir, "..");
const BIN = process.env.LIFIC_BIN ?? join(ROOT, "target", "debug", "lific");
const ADMIN_PASSWORD = "claim-admin-password-123";
const PASSWORD = "claim-user-password-123";
const SHOTS = process.env.LIFIC_SHOTS; // optional screenshot directory

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

/** Calls the REST API from inside the page with the session's token, as
 *  `worker` when given. Returns the status and the parsed body. */
async function call(
  page: Page,
  method: string,
  path: string,
  body?: unknown,
  worker?: string,
): Promise<{ status: number; body: any }> {
  return page.evaluate(
    async ([method, path, body, worker]) => {
      const headers: Record<string, string> = {
        Authorization: `Bearer ${localStorage.getItem("lific_token")}`,
        "Content-Type": "application/json",
      };
      if (worker) headers["X-Lific-Worker"] = worker;
      const res = await fetch(`/api${path}`, {
        method,
        headers,
        body: body === undefined ? undefined : JSON.stringify(body),
      });
      const text = await res.text();
      let parsed: unknown = text;
      try {
        parsed = JSON.parse(text);
      } catch {
        // not JSON
      }
      return { status: res.status, body: parsed };
    },
    [method, path, body, worker] as const,
  );
}

async function api(page: Page, method: string, path: string, body?: unknown, worker?: string): Promise<any> {
  const res = await call(page, method, path, body, worker);
  if (res.status >= 300) throw new Error(`${method} ${path}: ${res.status} ${JSON.stringify(res.body)}`);
  return res.body;
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

async function shot(page: Page, name: string) {
  if (SHOTS) await page.screenshot({ path: join(SHOTS, `${name}.png`), fullPage: true });
}

/** The worker id the server derives from an `X-Lific-Worker` header. */
function workerId(header: string): string {
  return `s-${createHash("sha256").update(header).digest("hex").slice(0, 8)}`;
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

  const scratch = mkdtempSync(join(tmpdir(), "lific-claims-"));
  const config = join(scratch, "lific.toml");
  const db = join(scratch, "claims.db");
  let server: ChildProcess | null = null;
  let browser: Browser | null = null;
  let serverLog = "";
  const deadline = setTimeout(() => {
    console.error("claims test deadline exceeded");
    server?.kill("SIGKILL");
    process.exit(1);
  }, 180_000);

  try {
    cli(config, db, [
      "init", "--no-service", "--json",
      "--name", "Claim Admin",
      "--auth-mode", "passwords",
      "--password", ADMIN_PASSWORD,
    ]);
    cli(config, db, ["project", "create", "--name", "Claims", "--identifier", "CLM", "--json"]);
    for (const username of ["mia", "ada"]) {
      cli(config, db, [
        "user", "create", "--username", username,
        "--email", `${username}@example.test`, "--password", PASSWORD,
      ]);
      cli(config, db, ["member", "add", "--project", "CLM", "--user", username, "--role", "maintainer"]);
    }

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

    const watcherContext = await browser.newContext({ reducedMotion: "reduce", viewport: { width: 1280, height: 900 } });
    const page = await signIn(watcherContext, base, "mia", PASSWORD);
    page.setDefaultTimeout(10_000);
    page.on("pageerror", (err) => errors.push(String(err)));
    const agentContext = await browser.newContext();
    const agent = await signIn(agentContext, base, "ada", PASSWORD);

    const projects = await api(page, "GET", "/projects");
    const project = projects.find((p: { identifier: string }) => p.identifier === "CLM");
    const create = (title: string) =>
      api(page, "POST", "/issues", { project_id: project.id, title, status: "todo" });
    const held = await create("Rotate the signing keys");
    const free = await create("Trim the release notes");

    // An agent session of ada's claims the issue.
    const claimed = await api(agent, "POST", `/issues/${held.id}/claim`, { minutes: 30 }, "agent-one");
    const worker = workerId("agent-one");
    assert.equal(claimed.claim?.username, "ada");
    assert.equal(claimed.claim?.worker, worker);
    assert.equal(claimed.status, "active", "claiming a todo issue starts it");
    console.log("ok   an agent session claims an issue over REST");

    const refused = await call(agent, "POST", `/issues/${held.id}/claim`, undefined, "agent-two");
    assert.equal(refused.status, 409, "a second worker is refused");
    assert.ok(
      JSON.stringify(refused.body).includes(`claimed by @ada (worker ${worker})`),
      `the refusal names the holder: ${JSON.stringify(refused.body)}`,
    );
    console.log("ok   a second worker on the same account is refused, naming the first");

    // Home: listed under "Claimed now", out of the agent queue.
    await page.goto(`${base}/`);
    const claimedNow = page.getByTestId("claimed-now");
    const heldRow = claimedNow.locator(`[data-identifier="${held.identifier}"]`);
    await heldRow.waitFor({ state: "visible", timeout: 15_000 });
    const chip = heldRow.getByTestId("claim-chip");
    assert.match((await chip.innerText()).replace(/\s+/g, " "), /^ada (29|30)m left$/);
    assert.ok(
      ((await chip.getAttribute("title")) ?? "").includes(`@ada (worker ${worker})`),
      "the chip's tooltip names the worker",
    );
    // The queue's own rows; "Claimed now" sits under it in the same lane.
    const queue = page.getByTestId("agent-queue").locator("ol");
    await queue.locator(`[data-identifier="${free.identifier}"]`).waitFor();
    assert.equal(await queue.locator(`[data-identifier="${held.identifier}"]`).count(), 0);
    await shot(page, "home-claimed");
    console.log("ok   Home lists the claim with its holder and keeps it out of the agent queue");

    // Issue view: the Claimed by field.
    await page.goto(`${base}/CLM/issues/${held.identifier}`);
    const field = page.getByTestId("issue-claim");
    await field.waitFor({ state: "visible", timeout: 15_000 });
    assert.equal(await field.getByTestId("issue-claim-holder").innerText(), "ada");
    assert.ok((await field.innerText()).includes(`worker ${worker}`), await field.innerText());
    await shot(page, "issue-claimed");
    console.log("ok   the issue view names the holder and the worker");

    // Released by the agent: the open issue view drops the field live.
    const released = await api(agent, "POST", `/issues/${held.id}/release`, undefined, "agent-one");
    assert.equal(released.claim, undefined);
    await field.waitFor({ state: "detached", timeout: 10_000 });
    console.log("ok   a release clears the issue view without a reload");

    // And Home, left open, drops the row.
    await page.goto(`${base}/`);
    await page.getByTestId("agent-queue").waitFor();
    await api(agent, "POST", `/issues/${free.id}/claim`, undefined, "agent-one");
    const freeRow = page.getByTestId("claimed-now").locator(`[data-identifier="${free.identifier}"]`);
    await freeRow.waitFor({ state: "visible", timeout: 15_000 });
    await api(agent, "POST", `/issues/${free.id}/release`, undefined, "agent-one");
    await page.getByTestId("claimed-now").waitFor({ state: "detached", timeout: 15_000 });
    console.log("ok   Home follows a claim and its release live");

    // 390px: the row and its chip stay on the screen.
    await api(agent, "POST", `/issues/${held.id}/claim`, undefined, "agent-one");
    const phoneContext = await browser.newContext({
      reducedMotion: "reduce",
      viewport: { width: 390, height: 844 },
      isMobile: true,
      hasTouch: true,
    });
    const phone = await signIn(phoneContext, base, "mia", PASSWORD);
    phone.on("pageerror", (err) => errors.push(String(err)));
    await phone.goto(`${base}/`);
    const phoneRow = phone.getByTestId("claimed-now").locator(`[data-identifier="${held.identifier}"]`);
    await phoneRow.waitFor({ state: "visible", timeout: 15_000 });
    const rowBox = await phoneRow.boundingBox();
    const chipBox = await phoneRow.getByTestId("claim-chip").boundingBox();
    assert.ok(rowBox && rowBox.x >= 0 && rowBox.x + rowBox.width <= 390, `row fits: ${JSON.stringify(rowBox)}`);
    assert.ok(chipBox && chipBox.x + chipBox.width <= 390, `chip fits: ${JSON.stringify(chipBox)}`);
    await phoneRow.scrollIntoViewIfNeeded();
    await shot(phone, "home-claimed-390");
    console.log("ok   the Home claim row fits 390px");

    assert.deepEqual(errors, [], "no uncaught page errors");
    console.log("\nissue claims test passed");
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

#!/usr/bin/env bun
/**
 * Cross-project relations from the issue view (LIF-504).
 *
 * The issue view's "Add relation" picker searches every project the reader
 * can see: current-project hits first, others labelled with their project.
 * This drives the real app against a scratch server and checks:
 *
 *   * a Blocks relation to another project's issue, found by title and by
 *     typed identifier, renders a chip that opens that issue;
 *   * the plan-step attach picker stays scoped to the plan's project;
 *   * a project the reader cannot view never shows up in the picker;
 *   * a reader who is only a Viewer on the other project sees the server's
 *     refusal instead of a silent no-op.
 *
 * Everything this script starts it owns and kills in `finally`: the server
 * is a child process and the scratch directory is removed.
 *
 * Run:  cargo build && cd e2e && bun cross-project-relations.ts
 * Binary picked: $LIFIC_BIN, else target/debug/lific (a debug build reads
 * web/dist from disk, so build the web UI first).
 */
import { chromium, type Browser, type Page } from "playwright";
import { spawn, execFileSync, type ChildProcess } from "node:child_process";
import { mkdtempSync, rmSync, existsSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const ROOT = resolve(import.meta.dir, "..");
const BIN = process.env.LIFIC_BIN ?? join(ROOT, "target", "debug", "lific");
const PASSWORD = "cross-project-password-123";

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
      } else {
        srv.close(() => rej(new Error("could not allocate a port")));
      }
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

/** Calls the REST API from inside the page, with the session's token. */
async function api(page: Page, path: string, body?: unknown): Promise<any> {
  return page.evaluate(
    async ([path, body]) => {
      const res = await fetch(`/api${path}`, {
        method: body === undefined ? "GET" : "POST",
        headers: {
          Authorization: `Bearer ${localStorage.getItem("lific_token")}`,
          "Content-Type": "application/json",
        },
        body: body === undefined ? undefined : JSON.stringify(body),
      });
      if (!res.ok) throw new Error(`${path}: ${res.status} ${await res.text()}`);
      return res.json();
    },
    [path, body] as const,
  );
}

async function login(page: Page, base: string, identity: string) {
  await page.goto(`${base}/login`, { waitUntil: "load", timeout: 15_000 });
  await page.fill("#login-identity", identity);
  await page.fill("#login-password", PASSWORD);
  await page.click("button[type=submit]");
  await page.waitForURL((url) => !url.pathname.startsWith("/login"), { timeout: 15_000 });
  await page.waitForFunction(() => !!localStorage.getItem("lific_token"));
}

function relationField(page: Page, label: string) {
  return page.locator(".issue-meta-field").filter({
    has: page.locator(".issue-meta-field-label", { hasText: new RegExp(`^${label}$`, "i") }),
  });
}

async function openAddRelation(page: Page, kind: string) {
  await page.getByRole("button", { name: "Add relation" }).click();
  await page.locator(".issue-meta-relations").getByRole("button", { name: kind, exact: true }).click();
  await page.getByPlaceholder(/Search issues in any project/).waitFor();
}

/** Identifiers listed in the open picker, in order. */
async function pickerHits(page: Page): Promise<string[]> {
  return page.locator("[data-idx] .font-mono").allInnerTexts();
}

async function main(): Promise<number> {
  if (!existsSync(BIN)) {
    console.error(`no binary at ${BIN}: run \`cargo build\` first (or set LIFIC_BIN)`);
    return 1;
  }
  if (!existsSync(join(ROOT, "web", "dist", "index.html"))) {
    console.error("web/dist/index.html missing: run `cd web && bun run build` first");
    return 1;
  }

  const scratch = mkdtempSync(join(tmpdir(), "lific-xproj-"));
  const config = join(scratch, "lific.toml");
  const db = join(scratch, "xproj.db");
  let server: ChildProcess | null = null;
  let browser: Browser | null = null;
  let serverLog = "";
  const deadline = setTimeout(() => {
    console.error("cross-project relations test deadline exceeded");
    server?.kill("SIGKILL");
    process.exit(1);
  }, 150_000);

  try {
    // ---- seed ----------------------------------------------------------
    cli(config, db, [
      "init", "--no-service", "--json",
      "--name", "Cross Operator",
      "--auth-mode", "passwords",
      "--password", PASSWORD,
    ]);
    cli(config, db, ["project", "create", "--name", "Engine", "--identifier", "ENG", "--json"]);
    cli(config, db, ["project", "create", "--name", "Game", "--identifier", "GAME", "--json"]);
    cli(config, db, ["project", "create", "--name", "Secret", "--identifier", "SEC", "--json"]);
    const issue = (project: string, title: string) =>
      JSON.parse(cli(config, db, ["issue", "create", "--project", project, "--title", title, "--json"]))
        .identifier as string;
    const engSource = issue("ENG", "Engine source issue");
    const engGamepad = issue("ENG", "Gamepad input layer");
    const gameGamepad = issue("GAME", "Gamepad rumble support");
    const gameNamed = issue("GAME", "Save slot thumbnails");
    issue("SEC", "Gamepad secret prototype");
    // A maintainer of ENG who can only view GAME and cannot see SEC at all.
    cli(config, db, [
      "user", "create", "--username", "viewer-on-game", "--email", "v@example.com",
      "--password", PASSWORD, "--json",
    ]);
    cli(config, db, ["member", "add", "--project", "ENG", "--user", "viewer-on-game", "--role", "maintainer", "--json"]);
    cli(config, db, ["member", "add", "--project", "GAME", "--user", "viewer-on-game", "--role", "viewer", "--json"]);

    // ---- server --------------------------------------------------------
    const port = await freePort();
    const base = `http://127.0.0.1:${port}`;
    server = spawn(
      BIN,
      ["--config", config, "--db", db, "start", "--port", String(port), "--host", "127.0.0.1"],
      { stdio: ["ignore", "pipe", "pipe"] },
    );
    server.stdout?.on("data", (d: Buffer) => (serverLog += d.toString()));
    server.stderr?.on("data", (d: Buffer) => (serverLog += d.toString()));
    await waitForServer(`${base}/`, 30_000);

    browser = await chromium.launch({ executablePath: process.env.PLAYWRIGHT_EXECUTABLE_PATH });

    // ---- 1. admin links across projects --------------------------------
    {
      const context = await browser.newContext({ viewport: { width: 1280, height: 900 } });
      const page = await context.newPage();
      page.setDefaultTimeout(10_000);
      const pageErrors: string[] = [];
      page.on("pageerror", (err) => pageErrors.push(String(err)));
      await login(page, base, "cross-operator");

      // By title: the current project's hit leads, the other project's hit
      // follows under a divider and carries its project label.
      await page.goto(`${base}/ENG/issues/${engSource}`);
      await openAddRelation(page, "Blocks");
      await page.keyboard.type("Gamepad");
      const gameHit = page.locator("[data-idx]", { hasText: "Gamepad rumble support" });
      await gameHit.waitFor();
      const byTitle = await pickerHits(page);
      if (byTitle[0] !== engGamepad || !byTitle.includes(gameGamepad)) {
        throw new Error(`title search order: expected ${engGamepad} first then ${gameGamepad}, got ${byTitle}`);
      }
      await page.getByText("Other projects", { exact: true }).waitFor();
      if (!(await gameHit.innerText()).includes("Game (GAME)")) {
        throw new Error(`other-project hit is not labelled with its project: ${await gameHit.innerText()}`);
      }
      await gameHit.click();
      const chip = relationField(page, "Blocks").getByRole("button", { name: gameGamepad, exact: true });
      await chip.waitFor();

      // By typed identifier.
      await openAddRelation(page, "Related");
      await page.keyboard.type(gameNamed);
      const named = page.locator('[data-idx="0"]', { hasText: "Save slot thumbnails" });
      await named.waitFor();
      await named.click();
      await relationField(page, "Related").getByRole("button", { name: gameNamed, exact: true }).waitFor();

      // The chip opens the other project's issue under that project's route.
      await chip.click();
      await page.waitForURL(new RegExp(`/GAME/issues/${gameGamepad}$`));
      await relationField(page, "Blocked by").getByRole("button", { name: engSource, exact: true }).waitFor();

      // The plan-step attach picker stays scoped to the plan's project.
      const projects = await api(page, "/projects");
      const eng = projects.find((p: { identifier: string }) => p.identifier === "ENG");
      const plan = await api(page, "/plans", {
        project_id: eng.id,
        title: "Input plan",
        steps: [{ title: "Wire up the gamepad" }],
      });
      await page.goto(`${base}/ENG/plans/${plan.id}`);
      await page.getByTitle("Link an issue", { exact: true }).first().click();
      await page.getByPlaceholder(/Search ENG issues/).waitFor();
      await page.keyboard.type("Gamepad");
      await page.locator("[data-idx]", { hasText: "Gamepad input layer" }).waitFor();
      await page.waitForTimeout(500); // let any slower response land
      const planHits = await pickerHits(page);
      if (planHits.some((id) => !id.startsWith("ENG-"))) {
        throw new Error(`plan-step picker listed issues outside ENG: ${planHits}`);
      }
      if ((await page.getByText("Other projects", { exact: true }).count()) !== 0) {
        throw new Error("plan-step picker shows an other-projects group");
      }
      await page.keyboard.press("Escape");

      if (pageErrors.length) throw new Error(`uncaught page errors: ${pageErrors.join("; ")}`);
      await context.close();
      console.log("ok   relation picker finds and links another project's issue by title and identifier");
      console.log("ok   cross-project chip opens the other issue; plan-step picker stays project-scoped");
    }

    // ---- 2. a Viewer on the other project is refused visibly -----------
    {
      const context = await browser.newContext({ viewport: { width: 1280, height: 900 } });
      const page = await context.newPage();
      page.setDefaultTimeout(10_000);
      const pageErrors: string[] = [];
      page.on("pageerror", (err) => pageErrors.push(String(err)));
      await login(page, base, "viewer-on-game");

      await page.goto(`${base}/ENG/issues/${engGamepad}`);
      await openAddRelation(page, "Blocks");
      await page.keyboard.type("Gamepad");
      const hit = page.locator("[data-idx]", { hasText: "Gamepad rumble support" });
      await hit.waitFor();
      await page.waitForTimeout(500); // let any slower response land
      const hits = await pickerHits(page);
      if (hits.some((id) => id.startsWith("SEC-"))) {
        throw new Error(`picker leaked an issue from a project the user cannot view: ${hits}`);
      }
      if ((await page.getByText("Secret", { exact: false }).count()) !== 0) {
        throw new Error("picker leaked the name of a project the user cannot view");
      }
      const linkResponse = page.waitForResponse((r) => r.url().endsWith("/api/issues/link"));
      await hit.click();
      const status = (await linkResponse).status();
      if (status !== 403) throw new Error(`expected the link to be refused with 403, got ${status}`);
      await page.getByText(new RegExp(`Couldn't link ${gameGamepad}:`)).waitFor();
      if ((await relationField(page, "Blocks").count()) !== 0) {
        throw new Error("a refused link still rendered a Blocks chip");
      }

      if (pageErrors.length) throw new Error(`uncaught page errors: ${pageErrors.join("; ")}`);
      await context.close();
      console.log("ok   hidden projects stay out of the picker; a refused link shows the server's error");
    }
    return 0;
  } catch (err) {
    console.error(`FAIL ${err instanceof Error ? err.stack ?? err.message : String(err)}`);
    if (serverLog) console.error(`---- server log (tail) ----\n${serverLog.slice(-3000)}`);
    return 1;
  } finally {
    clearTimeout(deadline);
    await browser?.close().catch(() => {});
    server?.kill("SIGTERM");
    rmSync(scratch, { recursive: true, force: true });
  }
}

process.exit(await main());

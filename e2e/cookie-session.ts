#!/usr/bin/env bun
/**
 * The web app's session lives only in the HttpOnly `lific_token` cookie.
 *
 * Drives a real server and Chromium through:
 *
 *   1. a token left in localStorage by an older version is removed on load,
 *   2. signing in through /login: the cookie is HttpOnly and SameSite=Lax,
 *      and neither localStorage nor document.cookie holds the session,
 *   3. a write through the UI (Settings, profile) on the cookie alone,
 *   4. the realtime socket opening on the cookie and delivering an event for
 *      an issue an API key created (API keys still authenticate as bearer),
 *   5. a POST from another origin on the same site, which the browser sends
 *      with the cookie, refused 403 by the origin check,
 *   6. signing out from Settings: the cookie is cleared and the session ends
 *      on the server.
 *
 * Run:   bun e2e/cookie-session.ts   (after `cargo build` and a web build)
 * Binary: $LIFIC_BIN, else target/debug/lific. Owns its server and exits.
 */
import { chromium, type Browser, type Page, type WebSocket } from "playwright";
import { spawn, execFileSync, type ChildProcess } from "node:child_process";
import { mkdtempSync, rmSync, existsSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const ROOT = resolve(import.meta.dir, "..");
const BIN = process.env.LIFIC_BIN ?? join(ROOT, "target", "debug", "lific");
const PASSWORD = "cookie-password-123";
const USER = "cookie-operator";

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

function check(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

async function meStatus(page: Page): Promise<number> {
  return page.evaluate(async () => (await fetch("/api/auth/me")).status);
}

async function main(): Promise<number> {
  if (!existsSync(BIN)) {
    console.error(`no binary at ${BIN}; run \`cargo build\` first (or set LIFIC_BIN)`);
    return 1;
  }
  if (!existsSync(join(ROOT, "web", "dist", "index.html"))) {
    console.error("web/dist/index.html missing; run `bun run build` in web/ first");
    return 1;
  }

  const scratch = mkdtempSync(join(tmpdir(), "lific-cookie-"));
  const config = join(scratch, "lific.toml");
  const db = join(scratch, "cookie.db");
  let server: ChildProcess | null = null;
  let browser: Browser | null = null;
  let serverLog = "";
  let step = "seed";

  try {
    cli(config, db, [
      "init", "--no-service", "--json",
      "--name", "Cookie Operator",
      "--auth-mode", "passwords",
      "--password", PASSWORD,
    ]);
    cli(config, db, ["project", "create", "--name", "Demo", "--identifier", "DEMO", "--json"]);
    const keyOut = JSON.parse(cli(config, db, ["key", "create", "--name", "script", "--user", USER, "--json"]));
    const apiKey: string = keyOut.key ?? keyOut.api_key ?? keyOut.token;
    check(typeof apiKey === "string" && apiKey.startsWith("lific_sk"), `no API key in ${JSON.stringify(keyOut)}`);

    const port = await freePort();
    // `localhost`, not 127.0.0.1: step 5 needs a second origin on the same
    // site, and the cookie is scoped to the host name, not the port.
    const base = `http://localhost:${port}`;
    server = spawn(BIN, ["--config", config, "--db", db, "start", "--port", String(port), "--host", "127.0.0.1"], {
      stdio: ["ignore", "pipe", "pipe"],
    });
    server.stdout?.on("data", (d: Buffer) => (serverLog += d.toString()));
    server.stderr?.on("data", (d: Buffer) => (serverLog += d.toString()));
    await waitForServer(`${base}/api/health`, 30_000);

    browser = await chromium.launch({ executablePath: process.env.PLAYWRIGHT_EXECUTABLE_PATH });
    const context = await browser.newContext();
    const page = await context.newPage();
    const pageErrors: string[] = [];
    page.on("pageerror", (err) => pageErrors.push(String(err)));

    // ---- 1. a leftover token from an older version is dropped ----------
    step = "leftover token";
    await page.goto(`${base}/login`, { waitUntil: "load", timeout: 15_000 });
    await page.evaluate(() => localStorage.setItem("lific_token", "lific_sess_left_over"));
    await page.reload({ waitUntil: "load" });
    await page.locator("#login-identity").waitFor({ timeout: 15_000 });
    check(
      (await page.evaluate(() => localStorage.getItem("lific_token"))) === null,
      "a token stored by an older version survived a load",
    );
    console.log("ok   a leftover localStorage token is removed on load");

    // ---- 2. sign in -----------------------------------------------------
    step = "sign in";
    let socket: WebSocket | null = null;
    page.on("websocket", (ws) => {
      if (ws.url().includes("/api/events/ws")) socket = ws;
    });
    await page.fill("#login-identity", USER);
    await page.fill("#login-password", PASSWORD);
    const loginResponse = page.waitForResponse((r) => r.url().endsWith("/api/auth/login"));
    await page.click("button[type=submit]");
    const login = await loginResponse;
    check(login.status() === 200, `login answered ${login.status()}`);
    const loginBody = await login.json();
    check(!("token" in loginBody), `the login body still carries a token: ${JSON.stringify(loginBody)}`);
    await page.waitForURL((url) => !url.hash.includes("login"), { timeout: 15_000 });

    const cookie = (await context.cookies(base)).find((c) => c.name === "lific_token");
    check(cookie, "no lific_token cookie after signing in");
    check(cookie.httpOnly, "the session cookie is readable from script (not HttpOnly)");
    check(cookie.sameSite === "Lax", `the session cookie is SameSite=${cookie.sameSite}`);
    check(cookie.value.startsWith("lific_sess_"), "the cookie does not hold a session");
    const visible = await page.evaluate(() => ({
      cookie: document.cookie,
      stored: Object.keys(localStorage).map((k) => `${k}=${localStorage.getItem(k)}`).join("\n"),
    }));
    check(!visible.cookie.includes("lific_sess_"), "document.cookie exposes the session");
    check(!visible.stored.includes("lific_sess_"), "localStorage holds a session token");
    check((await meStatus(page)) === 200, "/api/auth/me is not signed in on the cookie");
    console.log("ok   sign-in sets an HttpOnly SameSite=Lax cookie and leaves no token readable");

    // ---- 3. a write through the UI --------------------------------------
    step = "write";
    await page.goto(`${base}/#/settings`, { waitUntil: "load" });
    const nameInput = page.locator("label", { hasText: "Display name" }).locator("input").first();
    await nameInput.waitFor({ timeout: 15_000 });
    await nameInput.fill("Cookie Writer");
    const saved = page.waitForResponse(
      (r) => r.url().endsWith("/api/auth/me") && r.request().method() === "PATCH",
    );
    await page.getByRole("button", { name: "Save changes" }).click();
    const patch = await saved;
    check(patch.status() === 200, `profile save answered ${patch.status()}`);
    check(
      (await patch.request().allHeaders())["authorization"] === undefined,
      "the write carried a script-held Authorization header",
    );
    const displayName = await page.evaluate(async () => (await (await fetch("/api/auth/me")).json()).display_name);
    check(displayName === "Cookie Writer", `the profile write did not land: ${displayName}`);
    console.log("ok   a UI write succeeds on the cookie alone");

    // ---- 4. realtime socket and an API key ------------------------------
    step = "realtime";
    const deadline = Date.now() + 15_000;
    while ((!socket || socket.isClosed()) && Date.now() < deadline) await page.waitForTimeout(100);
    const live = socket as WebSocket | null;
    check(live && !live.isClosed(), "no realtime socket is open");
    const frame = live.waitForEvent("framereceived", {
      predicate: (f) => String(f.payload).includes("issue.created"),
      timeout: 15_000,
    });
    const projects = await (await fetch(`${base}/api/projects`, {
      headers: { Authorization: `Bearer ${apiKey}` },
    })).json();
    const project = projects.find((p: { identifier: string }) => p.identifier === "DEMO");
    check(project, `the API key cannot list DEMO: ${JSON.stringify(projects)}`);
    const created = await fetch(`${base}/api/issues`, {
      method: "POST",
      headers: { Authorization: `Bearer ${apiKey}`, "Content-Type": "application/json" },
      body: JSON.stringify({ project_id: project.id, title: "Made by a script" }),
    });
    check(created.status === 200, `API-key write answered ${created.status}: ${await created.text()}`);
    await frame;
    check(!live.isClosed(), "the realtime socket closed");
    console.log("ok   the realtime socket opens on the cookie and delivers an API-key write");

    // ---- 5. another origin on the same site -----------------------------
    step = "cross-origin";
    // Same site as `base` (localhost), different origin (another port), so
    // SameSite=Lax lets the browser attach the cookie and only the origin
    // check stands between this page and a write.
    const foreign = `http://localhost:${port + 1}`;
    await context.route(`${foreign}/**`, (route) =>
      route.fulfill({ status: 200, contentType: "text/html", body: "<!doctype html><title>elsewhere</title>" }),
    );
    const attacker = await context.newPage();
    await attacker.goto(`${foreign}/`);
    const forged = attacker.waitForRequest((r) => r.url() === `${base}/api/auth/logout`);
    const refused = attacker.waitForResponse((r) => r.url() === `${base}/api/auth/logout`);
    // A plain form post: no preflight, and its response is a navigation the
    // test can read (a no-cors fetch's response is blocked from view).
    await attacker.evaluate((target) => {
      const form = document.createElement("form");
      form.method = "POST";
      form.action = target;
      form.enctype = "text/plain";
      document.body.appendChild(form);
      form.submit();
    }, `${base}/api/auth/logout`);
    const forgedHeaders = await (await forged).allHeaders();
    check(
      (forgedHeaders["cookie"] ?? "").includes("lific_token="),
      "the browser did not send the cookie, so this step proves nothing",
    );
    check(forgedHeaders["origin"] === foreign, `unexpected Origin ${forgedHeaders["origin"]}`);
    check((await refused).status() === 403, `a cross-origin POST answered ${(await refused).status()}`);
    await attacker.close();
    check((await meStatus(page)) === 200, "the refused cross-origin logout still ended the session");
    console.log("ok   a cross-origin POST carrying the cookie is refused 403");

    // ---- 6. sign out ----------------------------------------------------
    step = "sign out";
    await page.goto(`${base}/#/settings`, { waitUntil: "load" });
    await page.getByRole("button", { name: "Sign out", exact: true }).click();
    await page.waitForURL((url) => url.hash.includes("login"), { timeout: 15_000 });
    check(
      !(await context.cookies(base)).some((c) => c.name === "lific_token"),
      "the session cookie survived signing out",
    );
    check((await meStatus(page)) === 401, "still signed in after signing out");
    const replayed = await fetch(`${base}/api/auth/me`, {
      headers: { Cookie: `lific_token=${cookie.value}` },
    });
    check(replayed.status === 401, `the old session still works after sign-out: ${replayed.status}`);
    console.log("ok   sign-out clears the cookie and ends the session on the server");

    check(pageErrors.length === 0, `uncaught page errors: ${pageErrors.join("; ")}`);
    console.log("\ncookie session e2e passed");
    return 0;
  } catch (e) {
    console.error(`FAIL ${step}: ${e instanceof Error ? e.message : e}`);
    if (serverLog.trim()) {
      console.error("\nlast server output:");
      console.error(serverLog.split("\n").slice(-25).join("\n"));
    }
    return 1;
  } finally {
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

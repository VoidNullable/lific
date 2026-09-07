#!/usr/bin/env bun
/**
 * Browser smoke test (LIF-429).
 *
 * svelte-check reported 0 errors on the code that broke every issue page in
 * v2.7.0 (LIF-428): the crash was a runtime reactive-effect loop, so only
 * actually running the app finds that class of bug. This script:
 *
 *   1. spawns the lific binary against a scratch config + DB on a free port,
 *   2. seeds a user, project, issue, comment, and page via the CLI,
 *   3. signs in through the real /login page (the session cookie it sets is
 *      also what authenticates the realtime WebSocket; injecting an API key
 *      into localStorage would leave the socket handshake failing 403),
 *   4. drives headless Chromium through the main SPA routes,
 *   5. fails if any route renders the <svelte:boundary> fallback, logs a
 *      console error, throws an uncaught exception, or is missing content
 *      the seed guarantees should be visible.
 *
 * Run locally:   bun install && bun run smoke        (from e2e/)
 * Binary picked: $LIFIC_BIN, else target/debug/lific (debug rust-embed reads
 * web/dist from disk at runtime, so `bun run build` in web/ first).
 *
 * Everything is owned by this process and torn down in `finally`; nothing
 * outlives the script.
 */
import { chromium, type Browser } from "playwright";
import { spawn, execFileSync, type ChildProcess } from "node:child_process";
import { mkdtempSync, rmSync, existsSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { checkDiscordFeedback } from "./discord-feedback";
import { checkPaletteSearch } from "./palette-search";
import { checkDuplicateRelations } from "./duplicate-relations";
import { Database } from "bun:sqlite";

const ROOT = resolve(import.meta.dir, "..");
const BIN = process.env.LIFIC_BIN ?? join(ROOT, "target", "debug", "lific");
const PASSWORD = "smoke-password-123";

/** Routes to drive. `expect` are substrings the fully rendered page must
 * contain — content the seed data guarantees. Routes without `expect` still
 * get the boundary/console/pageerror assertions. */
const ROUTES: { path: string; expect?: string[] }[] = [
  { path: "/", expect: ["Demo"] },
  { path: "/DEMO/overview", expect: ["Demo"] },
  { path: "/DEMO/issues", expect: ["Smoke issue"] },
  // Issue detail and page detail carry the comment-window logic that broke
  // in LIF-428; the comment body must actually render.
  { path: "/DEMO/issues/DEMO-1", expect: ["Smoke issue", "First smoke comment"] },
  { path: "/DEMO/pages/1", expect: ["Smoke page"] },
  { path: "/DEMO/board", expect: ["Smoke issue"] },
  { path: "/DEMO/graph" },
  { path: "/DEMO/files" },
  { path: "/settings", expect: ["Settings"] },
];

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
      const r = await fetch(url);
      if (r.ok) return;
    } catch {
      // not up yet
    }
    await new Promise((r) => setTimeout(r, 150));
  }
  throw new Error(`server did not answer at ${url} within ${timeoutMs}ms`);
}

async function main(): Promise<number> {
  if (!existsSync(BIN)) {
    console.error(`no binary at ${BIN} — run \`cargo build\` first (or set LIFIC_BIN)`);
    return 1;
  }
  if (!existsSync(join(ROOT, "web", "dist", "index.html"))) {
    console.error("web/dist/index.html missing — run `bun run build` in web/ first");
    return 1;
  }

  const scratch = mkdtempSync(join(tmpdir(), "lific-smoke-"));
  const config = join(scratch, "lific.toml");
  const db = join(scratch, "smoke.db");
  let server: ChildProcess | null = null;
  let browser: Browser | null = null;
  let serverLog = "";
  const failures: string[] = [];

  try {
    // ---- seed ----------------------------------------------------------
    cli(config, db, [
      "init", "--no-service", "--json",
      "--name", "Smoke Operator",
      "--auth-mode", "passwords",
      "--password", PASSWORD,
    ]);
    cli(config, db, ["project", "create", "--name", "Demo", "--identifier", "DEMO", "--json"]);
    const seedDb = new Database(db);
    seedDb.run("UPDATE projects SET emoji = ? WHERE identifier = 'DEMO'", ["Lucide:Terminal"]);
    seedDb.close();
    cli(config, db, [
      "issue", "create", "--project", "DEMO",
      "--title", "Smoke issue",
      "--description", "Seeded by the smoke test",
      "--json",
    ]);
    cli(config, db, ["issue", "update", "DEMO-1", "--status", "active", "--json"]);
    cli(config, db, ["issue", "create", "--project", "DEMO", "--title", "Second smoke issue", "--status", "active", "--json"]);
    cli(config, db, ["issue", "create", "--project", "DEMO", "--title", "Excluded smoke issue", "--status", "backlog", "--json"]);
    cli(config, db, ["comment", "add", "DEMO-1", "--content", "First smoke comment", "--json"]);
    const pageOut = JSON.parse(
      cli(config, db, [
        "page", "create", "--project", "DEMO",
        "--title", "Smoke page",
        "--content", "# Smoke page\n\nSeeded body with a [link](https://example.com).",
        "--json",
      ]),
    );
    if (pageOut.id !== 1) throw new Error(`expected seeded page id 1, got ${pageOut.id}`);

    // ---- server --------------------------------------------------------
    const port = await freePort();
    const base = `http://127.0.0.1:${port}`;
    server = spawn(BIN, ["--config", config, "--db", db, "start", "--port", String(port), "--host", "127.0.0.1"], {
      stdio: ["ignore", "pipe", "pipe"],
    });
    server.stdout?.on("data", (d: Buffer) => (serverLog += d.toString()));
    server.stderr?.on("data", (d: Buffer) => (serverLog += d.toString()));
    await waitForServer(`${base}/`, 30_000);

    // ---- browser -------------------------------------------------------
    browser = await chromium.launch();
    const context = await browser.newContext();

    // Sign in the way a person does. The login flow stores the bearer token
    // in localStorage AND receives the `lific_token` session cookie, which is
    // the only credential the realtime WebSocket accepts.
    {
      const page = await context.newPage();
      const loginErrors: string[] = [];
      page.on("pageerror", (err) => loginErrors.push(String(err)));
      await page.goto(`${base}/login`, { waitUntil: "load", timeout: 15_000 });
      await page.fill("#login-identity", "smoke-operator");
      await page.fill("#login-password", PASSWORD);
      await page.click("button[type=submit]");
      await page.waitForURL(`${base}/`, { timeout: 15_000 }).catch(() => {});
      if (!page.url().endsWith("/") || page.url().includes("/login")) {
        const body = await page.locator("body").innerText().catch(() => "");
        throw new Error(
          `login did not land on the app (still at ${page.url()}). ` +
            `Page said: ${body.slice(0, 300)}${loginErrors.length ? ` | errors: ${loginErrors.join("; ")}` : ""}`,
        );
      }
      await page.close();
    }

    for (const route of ROUTES) {
      const page = await context.newPage();
      const consoleErrors: string[] = [];
      const pageErrors: string[] = [];
      page.on("console", (msg) => {
        if (msg.type() === "error") consoleErrors.push(msg.text());
      });
      page.on("pageerror", (err) => pageErrors.push(String(err)));

      try {
        await page.goto(`${base}${route.path}`, { waitUntil: "load", timeout: 15_000 });
        // Let the SPA fetch and render; fall through on busy pages rather
        // than failing the route for a slow network-idle.
        await page.waitForLoadState("networkidle", { timeout: 10_000 }).catch(() => {});

        const bodyText = (await page.locator("body").innerText()) ?? "";

        // The <svelte:boundary> fallback in App.svelte (LIF-193). Its
        // presence means a route component threw during render.
        if (bodyText.includes("Something went wrong")) {
          failures.push(`${route.path}: rendered the error boundary fallback`);
        }
        if (bodyText.trim().length === 0) {
          failures.push(`${route.path}: page rendered no visible text at all`);
        }
        for (const expected of route.expect ?? []) {
          if (!bodyText.includes(expected)) {
            failures.push(`${route.path}: expected visible text ${JSON.stringify(expected)} not found`);
          }
        }
        // The boundary's onerror logs "[lific] route render failed:" — but
        // any console error on a fresh page load is a bug worth failing on.
        for (const err of consoleErrors) {
          failures.push(`${route.path}: console error: ${err}`);
        }
        for (const err of pageErrors) {
          failures.push(`${route.path}: uncaught page error: ${err}`);
        }
      } catch (e) {
        failures.push(`${route.path}: navigation failed: ${e}`);
      } finally {
        await page.close();
      }
      console.log(`${failures.some((f) => f.startsWith(`${route.path}:`)) ? "FAIL" : "ok  "} ${route.path}`);
    }

    await checkDiscordFeedback(context, base);

    // LIF-445: seeds its own PAL project, so it must run after the DEMO
    // assertions above.
    await checkPaletteSearch(context, base);
    await checkDuplicateRelations(context, base);

    // ---- deep-link back synthesis (LIF-434) ----------------------------
    // A detail view opened as the app's entry point gets a synthesized
    // parent-list history entry at boot, so the system back button goes
    // "up" to the list instead of leaving the app. Fresh page = fresh
    // history, which is exactly the deep-link case.
    {
      const scenario = "deep-link back";
      const page = await context.newPage();
      try {
        await page.goto(`${base}/DEMO/issues/DEMO-1`, { waitUntil: "load", timeout: 15_000 });
        await page.waitForLoadState("networkidle", { timeout: 10_000 }).catch(() => {});
        // goBack() resolves null for same-document (hash) navigations, so
        // the URL, not the response, is the assertion. The parent is the
        // issue list or the board, whichever layout localStorage last saw
        // (the board route above persisted "board" for this context).
        await page.goBack({ timeout: 5_000 }).catch(() => null);
        await page.waitForURL(/#\/DEMO\/(issues|board)$/, { timeout: 5_000 }).catch(() => {});
        if (!/#\/DEMO\/(issues|board)$/.test(page.url())) {
          failures.push(
            `${scenario}: back from deep-linked issue landed on ${page.url()}, ` +
              `not the issue list; parent entry was not synthesized`,
          );
        } else {
          // The list still has to fetch after the hash flips; wait for the
          // seeded issue to actually render rather than sampling the body.
          const rendered = await page
            .waitForFunction(() => document.body.innerText.includes("Smoke issue"), undefined, {
              timeout: 10_000,
            })
            .then(() => true)
            .catch(() => false);
          if (!rendered) {
            failures.push(`${scenario}: issue list after back did not render the seeded issue`);
          }
        }
      } catch (e) {
        failures.push(`${scenario}: ${e}`);
      } finally {
        await page.close();
      }
      console.log(`${failures.some((f) => f.startsWith(`${scenario}:`)) ? "FAIL" : "ok  "} ${scenario} (LIF-434)`);
    }
  } catch (e) {
    failures.push(`harness error: ${e instanceof Error ? (e.stack ?? e.message) : e}`);
  } finally {
    if (browser) await browser.close().catch(() => {});
    if (server && !server.killed) {
      server.kill("SIGTERM");
      // Give it a moment to exit cleanly, then make sure.
      await new Promise((r) => setTimeout(r, 500));
      if (server.exitCode === null) server.kill("SIGKILL");
    }
    rmSync(scratch, { recursive: true, force: true });
  }

  if (failures.length > 0) {
    console.error(`\nsmoke test FAILED (${failures.length} problem${failures.length === 1 ? "" : "s"}):`);
    for (const f of failures) console.error(`  - ${f}`);
    if (serverLog.trim()) {
      console.error("\nlast server output:");
      console.error(serverLog.split("\n").slice(-25).join("\n"));
    }
    return 1;
  }
  console.log(`\nsmoke test passed: ${ROUTES.length} routes clean`);
  return 0;
}

process.exit(await main());

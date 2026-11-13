#!/usr/bin/env bun
/**
 * Command palette design check: screenshots, contrast and the redesign's
 * behavior (selection marker, hint line, single-line rows, highlighting,
 * empty state, keycaps, scopes, preview, item actions, restore on reopen).
 *
 * Spawns the debug binary against a scratch config + DB, seeds two projects
 * through the REST API, signs in, and drives Chromium at 1440px and 390px in
 * light and dark. Every process is owned here and torn down in `finally`.
 *
 * Run:   cd e2e && bun palette-design.ts
 * Env:   PALETTE_SHOTS   screenshot directory (default /tmp/opencode)
 *        PALETTE_LABEL   screenshot name prefix (default "after")
 *        PALETTE_REPORT_ONLY=1  screenshots and contrast report, no assertions
 * Binary: $LIFIC_BIN, else target/debug/lific (debug rust-embed reads
 * web/dist from disk, so build the web UI first).
 */
import { strict as assert } from "node:assert";
import { chromium, type Browser, type BrowserContext, type Page } from "playwright";
import { spawn, execFileSync, type ChildProcess } from "node:child_process";
import { mkdtempSync, rmSync, existsSync, mkdirSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { accents, appearance } from "./sidebar-visual";

const ROOT = resolve(import.meta.dir, "..");
const BIN = process.env.LIFIC_BIN ?? join(ROOT, "target", "debug", "lific");
const PASSWORD = "palette-password-123";
const USER = "palette-operator";
const SHOTS = process.env.PALETTE_SHOTS ?? "/tmp/opencode";
const LABEL = process.env.PALETTE_LABEL ?? "after";
const REPORT_ONLY = process.env.PALETTE_REPORT_ONLY === "1";

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

type Seed = { lif: number; omn: number; retry: number; retryPage: number };

async function seed(page: Page): Promise<Seed> {
  const lif = (await api(page, "POST", "/projects", { name: "Lific", identifier: "LIF" })).id;
  const omn = (await api(page, "POST", "/projects", { name: "Omni", identifier: "OMN" })).id;
  const issue = async (project_id: number, title: string, status: string, priority: string, description = "") =>
    (await api(page, "POST", "/issues", { project_id, title, status, priority, description })).id as number;
  const retry = await issue(lif, "Retry with exponential backoff", "active", "high",
    "Retry failed requests with exponential backoff to reduce transient errors across agents.\n\nCap the delay at thirty seconds and add jitter so a fleet does not retry in lockstep.");
  await issue(lif, "Add retry limits to the webhook sender", "todo", "medium", "Stop after five attempts.");
  await issue(lif, "Improve agent error reporting", "backlog", "low");
  await issue(lif, "Cache metadata for faster loads", "done", "medium");
  await issue(lif, "Consolidate import pipelines", "todo", "low");
  await issue(omn, "Retry failed archive imports", "backlog", "low", "Archive imports fail on a flaky disk.");
  await api(page, "PUT", `/issues/${retry}`, { assignees: [USER] });
  const retryPage = (await api(page, "POST", "/pages", {
    project_id: lif, title: "Retry policy",
    content: "# Retry policy\n\nEvery outbound call retries with backoff and jitter.",
  })).id as number;
  await api(page, "POST", "/pages", { project_id: lif, title: "Architecture overview", content: "How the pieces fit." });
  await api(page, "POST", "/plans", { project_id: lif, title: "Reliability rollout" });
  return { lif, omn, retry, retryPage };
}

async function signIn(context: BrowserContext, base: string) {
  const page = await context.newPage();
  await page.goto(`${base}/login`, { waitUntil: "load" });
  await page.fill("#login-identity", USER);
  await page.fill("#login-password", PASSWORD);
  await page.click("button[type=submit]");
  await page.waitForURL((url) => !url.href.includes("login"), { timeout: 15_000 });
  await page.waitForFunction(() => localStorage.getItem("lific_token"), null, { timeout: 15_000 });
  return page;
}

function panel(page: Page) {
  return page.locator("[data-command-palette]");
}

async function openPalette(page: Page) {
  await page.keyboard.press("Control+k");
  await page.waitForFunction(() => {
    const el = document.activeElement as HTMLInputElement | null;
    return el?.tagName === "INPUT" && !!el.closest(".fixed.inset-0");
  }, null, { timeout: 5_000 });
  return page.locator(".fixed.inset-0 input").first();
}

async function closePalette(page: Page) {
  await page.keyboard.press("Escape");
  await page.waitForFunction(() => !document.querySelector(".fixed.inset-0 input"), null, { timeout: 5_000 });
}

async function rows(page: Page): Promise<string[]> {
  return page.evaluate(() =>
    [...document.querySelectorAll("[data-flat-idx]")].map((el) =>
      (el as HTMLElement).innerText.replace(/\s+/g, " ").trim(),
    ),
  );
}

/** Text and marker contrast inside the open palette, composited against the
 *  real painted backgrounds (same approach as sidebar-visual.ts). */
async function paletteContrast(page: Page) {
  return page.evaluate(() => {
    const root = (document.querySelector("[data-command-palette]") ??
      document.querySelector(".fixed.inset-0 input")!.closest("div.overflow-hidden"))!;
    const canvas = document.createElement("canvas");
    canvas.width = canvas.height = 1;
    const ctx = canvas.getContext("2d", { willReadFrequently: true })!;
    function rgb(color: string): number[] {
      ctx.clearRect(0, 0, 1, 1);
      ctx.fillStyle = color;
      ctx.fillRect(0, 0, 1, 1);
      return [...ctx.getImageData(0, 0, 1, 1).data];
    }
    function background(el: Element | null): number[] {
      if (!el) return [255, 255, 255, 255];
      const fg = rgb(getComputedStyle(el).backgroundColor);
      if (fg[3] === 255) return fg;
      const bg = background(el.parentElement), alpha = fg[3] / 255;
      return fg.slice(0, 3).map((v, i) => v * alpha + bg[i] * (1 - alpha)).concat(255);
    }
    function over(fg: number[], bg: number[]) {
      const alpha = fg[3] / 255;
      return fg.slice(0, 3).map((v, i) => v * alpha + bg[i] * (1 - alpha)).concat(255);
    }
    function ratio(a: number[], b: number[]) {
      const luminance = (c: number[]) => c.slice(0, 3).map((v) => {
        v /= 255;
        return v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4;
      }).reduce((sum, v, i) => sum + v * [0.2126, 0.7152, 0.0722][i], 0);
      const l1 = luminance(a), l2 = luminance(b);
      return (Math.max(l1, l2) + 0.05) / (Math.min(l1, l2) + 0.05);
    }
    const texts = [...root.querySelectorAll<HTMLElement>("*")].filter((el) =>
      el.getClientRects().length && getComputedStyle(el).visibility !== "hidden" &&
      [...el.childNodes].some((n) => n.nodeType === Node.TEXT_NODE && n.textContent?.trim()),
    ).map((el) => {
      const bg = background(el);
      return { label: el.textContent!.trim().slice(0, 50), ratio: ratio(over(rgb(getComputedStyle(el).color), bg), bg) };
    });
    // Placeholder text is real text too.
    const input = root.querySelector("input");
    if (input && !input.value) {
      const bg = background(input);
      texts.push({ label: "placeholder", ratio: ratio(over(rgb(getComputedStyle(input, "::placeholder").color), bg), bg) });
    }
    const selected = root.querySelector<HTMLElement>('[data-flat-idx][aria-selected="true"]');
    let marker: number | null = null;
    if (selected) {
      const fill = background(selected);
      const m = rgb(getComputedStyle(selected, "::before").backgroundColor);
      marker = ratio(over(m, fill), fill);
    }
    const weakest = texts.reduce((a, b) => (a.ratio < b.ratio ? a : b), { label: "", ratio: 99 });
    return { minimumText: weakest.ratio, weakestText: weakest.label, marker };
  });
}

async function shots(page: Page, base: string, width: number, height: number) {
  await page.setViewportSize({ width, height });
  for (const theme of ["light", "dark"]) {
    await page.goto(`${base}/LIF/issues`, { waitUntil: "load" });
    await page.waitForFunction(() => document.querySelectorAll("[data-issue-index]").length >= 5, null, { timeout: 15_000 });
    await appearance(page, theme, "indigo");
    const input = await openPalette(page);
    await input.fill(""); // reopening may restore the previous query
    await page.waitForTimeout(400);
    await page.screenshot({ path: join(SHOTS, `palette-${LABEL}-${width}-${theme}-empty.png`) });
    await input.fill("retry");
    await page.waitForTimeout(700);
    await page.screenshot({ path: join(SHOTS, `palette-${LABEL}-${width}-${theme}-query.png`) });
    await closePalette(page);
  }
}

async function contrastSweep(page: Page, base: string) {
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto(`${base}/LIF/issues`, { waitUntil: "load" });
  await page.waitForFunction(() => document.querySelectorAll("[data-issue-index]").length >= 5, null, { timeout: 15_000 });
  const failures: string[] = [];
  for (const theme of ["light", "dark"]) for (const accent of accents) {
    await appearance(page, theme, accent);
    for (const query of ["", "retry"]) {
      const input = await openPalette(page);
      await input.fill(query); // "" also clears a restored query
      await page.waitForTimeout(query ? 600 : 300);
      const r = await paletteContrast(page);
      const line = `${theme.padEnd(5)} ${accent.padEnd(6)} ${JSON.stringify(query).padEnd(7)} text ${r.minimumText.toFixed(2)} (${r.weakestText}) marker ${r.marker?.toFixed(2) ?? "none"}`;
      console.log(`     ${line}`);
      if (r.minimumText < 4.5) failures.push(`text < 4.5: ${line}`);
      if (r.marker === null || r.marker < 3) failures.push(`marker < 3: ${line}`);
      await closePalette(page);
    }
  }
  return failures;
}

async function behavior(page: Page, base: string, s: Seed) {
  await page.setViewportSize({ width: 1440, height: 900 });
  await appearance(page, "light", "indigo");
  // Recently opened items for the empty state.
  await page.goto(`${base}/LIF/issues/LIF-1`, { waitUntil: "load" });
  await page.getByText("Retry failed requests").first().waitFor();
  await page.goto(`${base}/LIF/pages/${s.retryPage}`, { waitUntil: "load" });
  await page.getByText("Every outbound call").first().waitFor();
  await page.goto(`${base}/LIF/issues`, { waitUntil: "load" });
  await page.waitForFunction(() => document.querySelectorAll("[data-issue-index]").length >= 5, null, { timeout: 15_000 });

  // Placeholder and hint line.
  let input = await openPalette(page);
  assert.equal(await input.getAttribute("placeholder"), "Search or jump to…");
  const hint = panel(page).locator("[data-palette-hint]");
  await hint.waitFor({ state: "visible" });

  // Empty state: recents and top actions, no project rows.
  await page.waitForTimeout(300);
  const empty = await rows(page);
  for (const want of ["New issue", "New page", "Switch project"]) {
    assert(empty.some((r) => r.includes(want)), `empty state lacks ${want}: ${JSON.stringify(empty)}`);
  }
  assert(empty.some((r) => r.includes("Retry policy")), `empty state lacks recent page: ${JSON.stringify(empty)}`);
  assert.equal(await panel(page).locator('[data-kind="project"]').count(), 0, "projects listed on an empty query");
  console.log("ok   empty state: recents and actions, no projects");

  // Typing: hint goes, rows are single-line, matches are highlighted.
  await input.fill("retry");
  await hint.waitFor({ state: "detached" });
  await page.waitForTimeout(500);
  const heights = await panel(page).locator('[data-flat-idx][data-kind="issue"]').evaluateAll((els) =>
    els.map((el) => el.getBoundingClientRect().height),
  );
  assert(heights.length >= 2, "expected issue rows for retry");
  for (const h of heights) assert(h >= 32 && h <= 40, `row height ${h} outside 32..40`);
  const marks = await panel(page).locator("[data-flat-idx] mark").allInnerTexts();
  assert(marks.some((m) => m.toLowerCase() === "retry"), `no highlighted match: ${JSON.stringify(marks)}`);
  const first = panel(page).locator('[data-flat-idx][aria-selected="true"]');
  assert.match(await first.innerText(), /LIF-1\s+Retry with exponential backoff/);
  console.log(`ok   rows: ${heights.length} single-line issue rows (${Math.min(...heights)}-${Math.max(...heights)}px), ${marks.length} highlights`);

  // Projects appear when matched.
  await input.fill("omni");
  await panel(page).locator('[data-kind="project"]').first().waitFor();
  console.log("ok   project row appears when its name is typed");

  // Scope chip cycles with Tab.
  await input.fill("");
  const chip = panel(page).locator("[data-palette-scope]");
  assert.equal((await chip.innerText()).trim(), "All");
  await page.keyboard.press("Tab");
  assert.equal((await chip.innerText()).trim(), "Issues");
  await input.fill("retry");
  await page.waitForTimeout(500);
  const kinds = await panel(page).locator("[data-flat-idx]").evaluateAll((els) => els.map((el) => el.getAttribute("data-kind")));
  assert(kinds.length > 0 && kinds.every((k) => k === "issue"), `Issues scope leaked: ${kinds}`);
  await input.fill("");
  for (let i = 0; i < 3; i++) await page.keyboard.press("Tab");
  assert.equal((await chip.innerText()).trim(), "Projects");
  await page.waitForTimeout(200);
  const projectKinds = await panel(page).locator("[data-flat-idx]").evaluateAll((els) => els.map((el) => el.getAttribute("data-kind")));
  assert(projectKinds.length === 2 && projectKinds.every((k) => k === "project"), `Projects scope: ${projectKinds}`);
  await page.keyboard.press("Shift+Tab");
  assert.equal((await chip.innerText()).trim(), "Plans");
  await page.waitForTimeout(400);
  assert((await rows(page)).some((r) => r.includes("Reliability rollout")), "Plans scope lacks the seeded plan");
  for (let i = 0; i < 3; i++) await page.keyboard.press("Tab");
  assert.equal((await chip.innerText()).trim(), "All");
  console.log("ok   scope chip: Tab and Shift+Tab cycle, results filter by kind");

  // Footer keycaps.
  const keys = await panel(page).locator("[data-palette-footer] kbd").allInnerTexts();
  assert(keys.includes("Esc") && keys.some((k) => /Ctrl/.test(k)), `footer keycaps: ${JSON.stringify(keys)}`);
  console.log(`ok   footer keycaps: ${keys.join(" ")}`);

  // Preview pane follows the selection.
  await input.fill("retry");
  await page.waitForTimeout(500);
  const preview = panel(page).locator("[data-palette-preview]");
  await preview.waitFor({ state: "visible" });
  assert.match(await preview.innerText(), /Retry with exponential backoff[\s\S]*Active[\s\S]*High[\s\S]*Lific[\s\S]*Retry failed requests/i);
  await page.keyboard.press("ArrowDown");
  await page.waitForFunction(() => document.querySelector("[data-palette-preview]")?.textContent?.includes("webhook sender"));
  console.log("ok   preview pane follows the selection");

  // Contextual actions on the selected issue: set its status.
  await input.fill("webhook sender");
  await page.waitForTimeout(500);
  await page.keyboard.press("Control+.");
  await panel(page).getByText("Set status…").waitFor();
  await page.screenshot({ path: join(SHOTS, `palette-${LABEL}-1440-light-actions.png`) });
  const actionRows = await rows(page);
  for (const want of ["Open", "Open in new tab", "Copy link", "Set status…", "Set priority…"]) {
    assert(actionRows.some((r) => r.startsWith(want)), `item actions lack ${want}: ${JSON.stringify(actionRows)}`);
  }
  await input.fill("status");
  await page.keyboard.press("Enter");
  await input.fill("done");
  await page.keyboard.press("Enter");
  const deadline = Date.now() + 8_000;
  let status = "";
  while (Date.now() < deadline) {
    status = (await api(page, "GET", "/issues/resolve/LIF-2").catch(() => null))?.status
      ?? (await api(page, "GET", "/issues?project_id=" + s.lif)).find((i: any) => i.identifier === "LIF-2")?.status;
    if (status === "done") break;
    await page.waitForTimeout(150);
  }
  assert.equal(status, "done", "Set status from the palette did not reach the server");
  console.log("ok   item actions: set LIF-2 to done from the palette");

  // Restore on reopen.
  input = await openPalette(page);
  await input.fill("cache");
  await page.waitForTimeout(400);
  await closePalette(page);
  input = await openPalette(page);
  assert.equal(await input.inputValue(), "cache", "query not restored on reopen");
  assert.equal(
    await input.evaluate((el: HTMLInputElement) => el.selectionStart === 0 && el.selectionEnd === el.value.length),
    true,
    "restored query should be selected so typing replaces it",
  );
  await closePalette(page);
  console.log("ok   reopen restores the last query");

  // Phone: no preview, no horizontal overflow, close button.
  await page.setViewportSize({ width: 390, height: 844 });
  input = await openPalette(page);
  await input.fill("retry");
  await page.waitForTimeout(500);
  assert.equal(await panel(page).locator("[data-palette-preview]").count(), 0, "preview shown at 390px");
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false, "overflow at 390px");
  await page.getByRole("button", { name: "Close search" }).click();
  console.log("ok   390px: no preview, no overflow");
}

async function main(): Promise<number> {
  if (!existsSync(BIN)) {
    console.error(`no binary at ${BIN}; run cargo build first (or set LIFIC_BIN)`);
    return 1;
  }
  if (!existsSync(join(ROOT, "web", "dist", "index.html"))) {
    console.error("web/dist/index.html missing; run `cd web && bun run build` first");
    return 1;
  }
  mkdirSync(SHOTS, { recursive: true });
  const scratch = mkdtempSync(join(tmpdir(), "lific-palette-"));
  const config = join(scratch, "lific.toml");
  const db = join(scratch, "palette.db");
  let server: ChildProcess | null = null;
  let browser: Browser | null = null;
  let serverLog = "";
  const failures: string[] = [];
  try {
    cli(config, db, ["init", "--no-service", "--json", "--name", "Palette Operator", "--auth-mode", "passwords", "--password", PASSWORD]);
    const port = await freePort();
    const base = `http://127.0.0.1:${port}`;
    server = spawn(BIN, ["--config", config, "--db", db, "start", "--port", String(port), "--host", "127.0.0.1"], {
      stdio: ["ignore", "pipe", "pipe"],
    });
    server.stdout?.on("data", (d: Buffer) => (serverLog += d.toString()));
    server.stderr?.on("data", (d: Buffer) => (serverLog += d.toString()));
    await waitForServer(`${base}/`, 30_000);

    browser = await chromium.launch({ executablePath: process.env.PLAYWRIGHT_EXECUTABLE_PATH });
    const context = await browser.newContext({ reducedMotion: "reduce" });
    const page = await signIn(context, base);
    const errors: string[] = [];
    page.on("pageerror", (e) => errors.push(String(e)));
    page.on("console", (m) => { if (m.type() === "error") errors.push(m.text()); });
    const s = await seed(page);

    await shots(page, base, 1440, 900);
    await shots(page, base, 390, 844);
    console.log(`ok   screenshots in ${SHOTS}/palette-${LABEL}-*.png`);

    console.log("     contrast (all six accents, light and dark):");
    const contrast = await contrastSweep(page, base);
    if (!REPORT_ONLY) failures.push(...contrast);

    if (!REPORT_ONLY) {
      try {
        await behavior(page, base, s);
      } catch (e) {
        failures.push(String(e));
      }
      if (errors.length) failures.push(`browser errors:\n${errors.join("\n")}`);
    }
  } catch (e) {
    failures.push(String(e));
    if (serverLog) console.error(serverLog.slice(-2000));
  } finally {
    await browser?.close().catch(() => {});
    server?.kill("SIGTERM");
    rmSync(scratch, { recursive: true, force: true });
  }
  if (failures.length) {
    console.error(`FAIL palette design:\n- ${failures.join("\n- ")}`);
    return 1;
  }
  console.log("PASS palette design");
  return 0;
}

const deadline = setTimeout(() => { console.error("palette design deadline exceeded"); process.exit(1); }, 240_000);
const code = await main();
clearTimeout(deadline);
process.exit(code);

#!/usr/bin/env bun
/**
 * LIF-147 / LIF-506: say who must do an issue, from the web UI.
 *
 * Spawns the debug binary against a scratch config + DB, signs in through
 * /login as a project maintainer and as a viewer, and checks that:
 *
 *   - the issue view's Assignee field moves an issue to any person, to two
 *     named people and back to agents, with REST agreeing each time;
 *   - list rows and board cards carry the marker, unassigned ones none;
 *   - the list filter narrows to Me, Needs a person and Any agent;
 *   - the bulk bar assigns two issues to any person;
 *   - the activity timeline reads the assign and unassign entries;
 *   - Home's "Needs you" box lists an issue assigned to the user, one for
 *     any person and one waiting on them, and drops a row once cleared;
 *   - a viewer sees the assignment read-only;
 *   - the picker works at 390px, as a bottom sheet over the details drawer.
 *
 * Run:  cd e2e && bun issue-assignees.ts
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
const ADMIN_PASSWORD = "assign-admin-password-123";
const PASSWORD = "assign-user-password-123";
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

async function shot(page: Page, name: string) {
  if (SHOTS) await page.screenshot({ path: join(SHOTS, `${name}.png`) });
}

interface Stored {
  needs_human: boolean;
  assignees: string[];
}

async function stored(page: Page, id: number): Promise<Stored> {
  const issue = await api(page, "GET", `/issues/${id}`);
  return {
    needs_human: issue.needs_human,
    assignees: (issue.assignees ?? []).map((a: { username: string }) => a.username).sort(),
  };
}

/** Polls REST until the issue's assignment is `expected`. */
async function expectStored(page: Page, id: number, expected: Stored, what: string) {
  const deadline = Date.now() + 8_000;
  let last: Stored | null = null;
  while (Date.now() < deadline) {
    last = await stored(page, id);
    if (JSON.stringify(last) === JSON.stringify(expected)) return;
    await page.waitForTimeout(100);
  }
  assert.deepEqual(last, expected, what);
}

function picker(page: Page) {
  return page.getByTestId("assignee-picker");
}

async function openFieldPicker(page: Page) {
  await page.getByTestId("issue-assignee").getByRole("button").click();
  await picker(page).waitFor({ state: "visible" });
}

async function pickPeople(page: Page, usernames: string[]) {
  const dialog = picker(page);
  for (const name of usernames) {
    const option = dialog.locator(`[data-assign-person="${name}"]`);
    await option.waitFor({ state: "visible" });
    if ((await option.getAttribute("aria-checked")) !== "true") await option.click();
  }
  await dialog.getByTestId("assign-apply").click();
  await dialog.waitFor({ state: "detached" });
}

async function choose(page: Page, kind: "agents" | "human") {
  await picker(page).locator(`[data-assign-choice="${kind}"]`).click();
  await picker(page).waitFor({ state: "detached" });
}

/** Identifiers of the rows the list currently renders. */
async function listedRows(page: Page): Promise<string[]> {
  return page
    .locator('[role="group"][aria-label]')
    .evaluateAll((els) => els.map((el) => el.getAttribute("aria-label") ?? ""))
    .then((labels) => labels.filter((l) => /^[A-Z]+-\d+$/.test(l)));
}

async function expectRows(page: Page, present: string[], absent: string[], what: string) {
  const deadline = Date.now() + 8_000;
  let rows: string[] = [];
  while (Date.now() < deadline) {
    rows = await listedRows(page);
    if (present.every((p) => rows.includes(p)) && absent.every((a) => !rows.includes(a))) return;
    await page.waitForTimeout(100);
  }
  assert.fail(`${what}: rows ${JSON.stringify(rows)}, wanted ${JSON.stringify(present)} without ${JSON.stringify(absent)}`);
}

async function setFilter(page: Page, value: string) {
  await page.getByRole("button", { name: /^Filter/ }).first().click();
  const section = page.getByTestId("filter-assignee");
  await section.waitFor({ state: "visible" });
  if (value === "") await section.getByRole("button", { name: /^Any No assignee filter/ }).click();
  else await section.locator(`[data-assignee-filter="${value}"]`).click();
  await page.getByRole("button", { name: "Close filters" }).click();
  await section.waitFor({ state: "detached" });
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

  const scratch = mkdtempSync(join(tmpdir(), "lific-assign-"));
  const config = join(scratch, "lific.toml");
  const db = join(scratch, "assign.db");
  let server: ChildProcess | null = null;
  let browser: Browser | null = null;
  let serverLog = "";
  const deadline = setTimeout(() => {
    console.error("assignee test deadline exceeded");
    server?.kill("SIGKILL");
    process.exit(1);
  }, 240_000);

  try {
    cli(config, db, [
      "init", "--no-service", "--json",
      "--name", "Assign Admin",
      "--auth-mode", "passwords",
      "--password", ADMIN_PASSWORD,
    ]);
    cli(config, db, ["project", "create", "--name", "Assign", "--identifier", "ASGN", "--json"]);
    for (const [username, role] of [["mia", "maintainer"], ["ada", "maintainer"], ["vic", "viewer"]]) {
      cli(config, db, [
        "user", "create", "--username", username,
        "--email", `${username}@example.test`, "--password", PASSWORD,
      ]);
      cli(config, db, ["member", "add", "--project", "ASGN", "--user", username, "--role", role]);
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

    // ---- maintainer, desktop ----------------------------------------------
    const context = await browser.newContext({ reducedMotion: "reduce", viewport: { width: 1280, height: 860 } });
    const page = await signIn(context, base, "mia", PASSWORD);
    page.setDefaultTimeout(10_000);
    page.on("pageerror", (err) => errors.push(String(err)));

    const projects = await api(page, "GET", "/projects");
    const project = projects.find((p: { identifier: string }) => p.identifier === "ASGN");
    const create = (title: string) =>
      api(page, "POST", "/issues", { project_id: project.id, title, status: "todo" });
    const lead = await create("Decide the launch date");
    const bulkA = await create("Sign the lease");
    const bulkB = await create("Call the bank");
    const waiting = await create("Approve the budget");
    await api(page, "POST", `/issues/${waiting.id}/waits`, { user: "mia", note: "Sign off" });

    // Issue view: any person, two people, back to agents.
    await page.goto(`${base}/ASGN/issues/${lead.identifier}`);
    const field = page.getByTestId("issue-assignee");
    await field.filter({ hasText: "Any agent" }).waitFor();
    await openFieldPicker(page);
    await shot(page, "picker-desktop");
    await choose(page, "human");
    await expectStored(page, lead.id, { needs_human: true, assignees: [] }, "issue marked for any person");
    await field.filter({ hasText: "Any person" }).waitFor();
    console.log("ok   issue view marks an issue for any person");

    await openFieldPicker(page);
    await pickPeople(page, ["mia", "ada"]);
    await expectStored(page, lead.id, { needs_human: true, assignees: ["ada", "mia"] }, "issue names two people");
    await field.filter({ hasText: "ada" }).filter({ hasText: "mia" }).waitFor();
    console.log("ok   issue view names two people");

    // List row and board card markers.
    await page.goto(`${base}/ASGN/issues`);
    const row = page.getByRole("group", { name: lead.identifier, exact: true });
    const rowMarker = row.locator('.assignee-marker[data-assignment="people"]');
    await rowMarker.waitFor({ state: "visible", timeout: 15_000 });
    const rowLabel = (await rowMarker.getAttribute("aria-label")) ?? "";
    assert.ok(rowLabel.includes("ada") && rowLabel.includes("mia"), `row marker names both: ${rowLabel}`);
    assert.equal(
      await page.getByRole("group", { name: bulkA.identifier, exact: true }).locator(".assignee-marker").count(),
      0,
      "an unassigned row has no marker",
    );
    console.log("ok   list row shows named people; unassigned rows show nothing");

    // Filters: Me, Needs a person, Any agent.
    await setFilter(page, "me");
    await expectRows(page, [lead.identifier], [bulkA.identifier, waiting.identifier], "Me filter");
    await setFilter(page, "human");
    await expectRows(page, [lead.identifier], [bulkA.identifier], "Needs a person filter");
    await setFilter(page, "none");
    await expectRows(page, [bulkA.identifier, bulkB.identifier], [lead.identifier], "Any agent filter");
    await setFilter(page, "");
    await expectRows(page, [lead.identifier, bulkA.identifier], [], "no assignee filter");
    console.log("ok   list filters by Me, Needs a person and Any agent");

    // Bulk: two issues to any person.
    await page.getByRole("checkbox", { name: `Select ${bulkA.identifier}`, exact: true }).click();
    await page.getByRole("checkbox", { name: `Select ${bulkB.identifier}`, exact: true }).click();
    await page.getByRole("button", { name: "Assign", exact: true }).click();
    await picker(page).waitFor({ state: "visible" });
    await choose(page, "human");
    await expectStored(page, bulkA.id, { needs_human: true, assignees: [] }, "bulk A for any person");
    await expectStored(page, bulkB.id, { needs_human: true, assignees: [] }, "bulk B for any person");
    await page
      .getByRole("group", { name: bulkA.identifier, exact: true })
      .locator('.assignee-marker[data-assignment="human"]')
      .waitFor({ state: "visible" });
    console.log("ok   bulk bar assigns two issues to any person");

    await page.goto(`${base}/ASGN/board`);
    const card = page.locator("article").filter({ hasText: "Decide the launch date" });
    await card.locator('.assignee-marker[data-assignment="people"]').waitFor({ state: "visible", timeout: 15_000 });
    await page
      .locator("article")
      .filter({ hasText: "Sign the lease" })
      .locator('.assignee-marker[data-assignment="human"]')
      .waitFor({ state: "visible" });
    await shot(page, "board");
    console.log("ok   board cards show the people and any-person markers");

    // Home: Needs you.
    await page.goto(`${base}/`);
    const needs = page.getByTestId("needs-you");
    await needs.locator(`[data-attention-group="assigned"] [data-identifier="${lead.identifier}"]`).waitFor();
    await needs.locator(`[data-attention-group="human"] [data-identifier="${bulkA.identifier}"]`).waitFor();
    await needs.locator(`[data-attention-group="waiting"] [data-identifier="${waiting.identifier}"]`).waitFor();
    await shot(page, "home-needs-you");
    console.log("ok   Home lists issues assigned to you, for any person and waiting on you");

    // Back to agents from the issue view, then the activity timeline.
    await page.goto(`${base}/ASGN/issues/${lead.identifier}`);
    await field.filter({ hasText: "mia" }).waitFor();
    await openFieldPicker(page);
    await choose(page, "agents");
    await expectStored(page, lead.id, { needs_human: false, assignees: [] }, "issue back to agents");
    await field.filter({ hasText: "Any agent" }).waitFor();
    console.log("ok   issue view hands an issue back to agents");

    const activity = page.locator("section", { has: page.getByRole("heading", { name: "Activity" }) });
    await activity.getByRole("button", { name: /^Show all/ }).click().catch(() => {});
    for (const line of ["marked for any person", "assigned @ada", "assigned @mia", "removed the any-person mark", "unassigned @mia"]) {
      await activity.getByText(line, { exact: false }).first().waitFor();
    }
    console.log("ok   activity timeline reads assign and unassign entries");

    await page.goto(`${base}/`);
    await needs.locator(`[data-attention-group="human"] [data-identifier="${bulkA.identifier}"]`).waitFor();
    assert.equal(
      await needs.locator(`[data-identifier="${lead.identifier}"]`).count(),
      0,
      "a cleared issue leaves Needs you",
    );
    console.log("ok   Home drops an issue once its assignment is cleared");

    // ---- viewer -------------------------------------------------------------
    await api(page, "PUT", `/issues/${lead.id}`, { assignees: ["ada"] });
    const viewerContext = await browser.newContext({ reducedMotion: "reduce" });
    const viewer = await signIn(viewerContext, base, "vic", PASSWORD);
    viewer.setDefaultTimeout(10_000);
    viewer.on("pageerror", (err) => errors.push(String(err)));
    await viewer.goto(`${base}/ASGN/issues/${lead.identifier}`);
    const viewerField = viewer.getByTestId("issue-assignee");
    await viewerField.filter({ hasText: "ada" }).waitFor();
    assert.equal(await viewerField.getByRole("button").isDisabled(), true, "viewer field is read-only");
    await viewerField.click({ force: true });
    await viewer.waitForTimeout(300);
    assert.equal(await picker(viewer).count(), 0, "viewer gets no picker");
    console.log("ok   viewer sees the assignment read-only");

    // ---- maintainer, 390px ----------------------------------------------------
    const phone = await browser.newContext({
      reducedMotion: "reduce",
      viewport: { width: 390, height: 844 },
      hasTouch: true,
      isMobile: true,
    });
    const mobile = await signIn(phone, base, "mia", PASSWORD);
    mobile.setDefaultTimeout(10_000);
    mobile.on("pageerror", (err) => errors.push(String(err)));
    await mobile.goto(`${base}/ASGN/issues/${bulkB.identifier}`);
    await mobile.getByRole("button", { name: "Show details" }).click();
    const mobileField = mobile.getByTestId("issue-assignee");
    await mobileField.filter({ hasText: "Any person" }).waitFor();
    await openFieldPicker(mobile);
    const box = await picker(mobile).boundingBox();
    assert.ok(box, "picker has a box");
    assert.ok(box.width >= 389 && box.x <= 1, `picker spans the phone width: ${JSON.stringify(box)}`);
    assert.ok(Math.abs(box.y + box.height - 844) <= 1, `picker sits at the bottom: ${JSON.stringify(box)}`);
    await shot(mobile, "picker-390");
    await pickPeople(mobile, ["mia"]);
    await expectStored(mobile, bulkB.id, { needs_human: true, assignees: ["mia"] }, "assigned at 390px");
    await mobileField.filter({ hasText: "mia" }).waitFor();
    console.log("ok   picker works as a bottom sheet at 390px");

    // The row keeps its title readable with a marker at 390px.
    await mobile.goto(`${base}/ASGN/issues`);
    const phoneRow = mobile.getByRole("group", { name: bulkB.identifier, exact: true });
    await phoneRow.locator('.assignee-marker[data-assignment="people"]').waitFor({ state: "visible", timeout: 15_000 });
    const rowBox = await phoneRow.boundingBox();
    assert.ok(rowBox && rowBox.width <= 390, `row fits the phone: ${JSON.stringify(rowBox)}`);
    await shot(mobile, "list-390");
    console.log("ok   list rows keep the marker inside 390px");

    assert.deepEqual(errors, [], "no uncaught page errors");
    console.log("\nissue assignee test passed");
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

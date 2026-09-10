#!/usr/bin/env bun
// Scratch instances only. Build web/ and cargo build before running.
import { chromium, type Browser, type Page } from "playwright";
import { execFileSync, spawn, type ChildProcess } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { Database } from "bun:sqlite";
import assert from "node:assert/strict";

const ROOT = resolve(import.meta.dir, "..");
const BIN = join(ROOT, "target/debug/lific");
const PASSWORD = "archive-smoke-password-123";
const scratch = mkdtempSync(join(tmpdir(), "lific-archives-"));
const children: ChildProcess[] = [];
let browser: Browser | null = null;
let serverLog = "";
const errors: string[] = [];
const watchdog = setTimeout(() => { void finish(1, "Archive browser smoke exceeded 180 seconds"); }, 180_000);
let finishing = false;

async function finish(code: number, message?: string) {
  if (finishing) return;
  finishing = true;
  clearTimeout(watchdog);
  if (message) console.error(message);
  await browser?.close().catch(() => {});
  await Promise.all(children.map((child) => new Promise<void>((done) => {
    if (child.exitCode !== null) return done();
    child.once("exit", () => done());
    child.kill("SIGTERM");
    setTimeout(() => { child.kill("SIGKILL"); done(); }, 2000).unref();
  })));
  rmSync(scratch, { recursive: true, force: true });
  if (code && serverLog) console.error(serverLog.slice(-6000));
  process.exit(code);
}

async function freePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const server = createServer();
    server.on("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      assert(address && typeof address === "object");
      server.close(() => resolve(address.port));
    });
  });
}

async function instance(name: string, seed: boolean) {
  const directory = mkdtempSync(join(scratch, `${name}-`));
  const config = join(directory, "lific.toml");
  const db = join(directory, "lific.db");
  const cli = (args: string[]) => execFileSync(BIN, ["--config", config, "--db", db, ...args], { encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] });
  cli(["init", "--no-service", "--json", "--name", `Archive ${name}`, "--auth-mode", "passwords", "--password", PASSWORD]);
  cli(["user", "create", "--username", "regular", "--email", `regular-${name}@example.test`, "--password", PASSWORD, "--json"]);
  cli(["user", "create", "--username", "second-admin", "--email", `second-${name}@example.test`, "--password", PASSWORD, "--admin", "--json"]);
  if (seed) {
    cli(["project", "create", "--name", "Archive specimen", "--identifier", "ARC", "--json"]);
    cli(["issue", "create", "--project", "ARC", "--title", "Archive graph root", "--description", "Missing source file: /api/attachments/999999", "--json"]);
    cli(["issue", "create", "--project", "ARC", "--title", "Archive graph child", "--json"]);
    cli(["comment", "add", "ARC-1", "--content", "A comment carried across instances", "--json"]);
    cli(["page", "create", "--project", "ARC", "--title", "Archive notebook", "--content", "A page carried across instances", "--json"]);
  }
  const sql = new Database(db);
  const admin = sql.query("SELECT username FROM users WHERE is_admin=1 AND is_bot=0").get() as { username: string };
  sql.run("UPDATE instance_settings SET web_auto_login=0, authz_enforced=0");
  if (seed) sql.run("UPDATE projects SET is_public=1 WHERE identifier='ARC'");
  sql.close();
  const port = await freePort();
  const base = `http://127.0.0.1:${port}`;
  const child = spawn(BIN, ["--config", config, "--db", db, "start", "--host", "127.0.0.1", "--port", String(port)], { stdio: ["ignore", "pipe", "pipe"] });
  children.push(child);
  child.stdout?.on("data", (chunk) => { serverLog += chunk.toString(); });
  child.stderr?.on("data", (chunk) => { serverLog += chunk.toString(); });
  const deadline = Date.now() + 20_000;
  while (true) {
    try { if ((await fetch(base)).ok) break; } catch { /* starting */ }
    if (Date.now() > deadline) throw new Error(`${name} server did not start`);
    await Bun.sleep(100);
  }
  const login = async (identity: string) => {
    const res = await fetch(`${base}/api/auth/login`, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ identity, password: PASSWORD }) });
    assert.equal(res.status, 200, await res.clone().text());
    return (await res.json()).token as string;
  };
  const token = await login(admin.username);
  const api = async (path: string, method = "GET", body?: unknown) => {
    const res = await fetch(`${base}/api${path}`, { method, headers: { Authorization: `Bearer ${token}`, "Content-Type": "application/json" }, body: body === undefined ? undefined : JSON.stringify(body) });
    assert(res.ok, `${method} ${path}: ${res.status} ${await res.clone().text()}`);
    assert(res.headers.get("content-type")?.includes("application/json"), `${method} ${path} did not return JSON. Rebuild the backend with the archive routes before running this test.`);
    return res.json();
  };
  return { base, token, api, login };
}

function watch(page: Page) {
  page.on("pageerror", (error) => errors.push(String(error)));
  page.on("console", (message) => {
    if (message.type() !== "error") return;
    // Chromium reports intentional HTTP rejections and the lost-response probe.
    if (/^Failed to load resource:.*(400|403|409|422|net::ERR_FAILED)/.test(message.text())) return;
    errors.push(message.text());
  });
}

async function signedPage(base: string, token: string, width = 1280) {
  const context = await browser!.newContext({ viewport: { width, height: 900 } });
  await context.addCookies([{ name: "lific_token", value: token, url: base, httpOnly: true, sameSite: "Lax" }]);
  await context.route("https://fonts.googleapis.com/**", (route) => route.fulfill({ status: 200, contentType: "text/css", body: "" }));
  await context.addInitScript((token) => {
    if (!localStorage.getItem("lific_token")) localStorage.setItem("lific_token", token);
  }, token);
  const page = await context.newPage();
  watch(page);
  await page.goto(`${base}/projects/new`);
  return page;
}

async function visible(page: Page, text: string) {
  try {
    await page.getByText(text, { exact: false }).filter({ visible: true }).first().waitFor({ state: "visible", timeout: 15_000 });
  } catch (error) {
    throw new Error(`${error}\nPage: ${await page.locator("body").innerText()}`);
  }
}

async function changeSession(page: Page, base: string, token: string) {
  await page.context().addCookies([{ name: "lific_token", value: token, url: base, httpOnly: true, sameSite: "Lax" }]);
  await page.evaluate((token) => {
    const oldValue = localStorage.getItem("lific_token");
    localStorage.setItem("lific_token", token);
    window.dispatchEvent(new StorageEvent("storage", { key: "lific_token", oldValue, newValue: token }));
  }, token);
}

async function staleDownload(page: Page, base: string, leave: () => Promise<void>) {
  await page.goto(`${base}/ARC/overview`);
  const panel = page.locator("section").filter({ has: page.getByRole("heading", { name: "Project archive", exact: true }) });
  await panel.getByRole("checkbox").check();
  let release!: () => void, arrived!: () => void, finished!: () => void;
  const held = new Promise<void>((resolve) => { release = resolve; });
  const started = new Promise<void>((resolve) => { arrived = resolve; });
  const served = new Promise<void>((resolve) => { finished = resolve; });
  const downloads: string[] = [];
  const download = (event: { suggestedFilename: () => string }) => downloads.push(event.suggestedFilename());
  page.on("download", download);
  const aborted = page.waitForEvent("requestfailed", { predicate: (request) => request.url().endsWith("/api/project-archives/ARC"), timeout: 10_000 });
  await page.route("**/api/project-archives/ARC", async (route) => {
    const response = await route.fetch();
    arrived();
    await held;
    await route.fulfill({ response }).catch(() => {});
    finished();
  });
  await panel.getByRole("button", { name: "Download project archive" }).click();
  await started;
  await leave();
  release();
  await served;
  await aborted;
  await page.waitForTimeout(250);
  assert.deepEqual(downloads, [], "stale archive never triggers a browser download");
  page.off("download", download);
  await page.unroute("**/api/project-archives/ARC");
}

async function main() {
  const a = await instance("source", true);
  const b = await instance("destination", false);
  const caps = await b.api("/project-archives");
  assert.equal(caps.can_import, true, "fresh backend must expose archive capabilities");
  assert.deepEqual(await b.api("/projects"), [], "destination starts with no projects");
  await a.api("/issues/link", "POST", { source: "ARC-1", target: "ARC-2", relation_type: "blocks" });
  const issue = await a.api("/issues/resolve/ARC-1");
  const form = new FormData();
  form.append("file", new File(["Archive attachment bytes"], "archive-note.txt", { type: "text/plain" }));
  form.append("entity_type", "issue");
  form.append("entity_id", String(issue.id));
  const uploaded = await fetch(`${a.base}/api/attachments`, { method: "POST", headers: { Authorization: `Bearer ${a.token}` }, body: form });
  assert(uploaded.ok, await uploaded.text());

  browser = await chromium.launch();
  const source = await signedPage(a.base, a.token);
  await source.goto(`${a.base}/ARC/overview`);
  const exportPanel = source.locator("section").filter({ has: source.getByRole("heading", { name: "Project archive", exact: true }) });
  await exportPanel.getByRole("checkbox").check();
  const downloaded = source.waitForEvent("download");
  await exportPanel.getByRole("button", { name: "Download project archive" }).click();
  const archive = await downloaded;
  assert.equal(archive.suggestedFilename(), "ARC.lific.tar.gz");
  const archivePath = join(scratch, archive.suggestedFilename());
  await archive.saveAs(archivePath);
  assert.equal(await archive.failure(), null);
  assert(Bun.gunzipSync(await Bun.file(archivePath).bytes()).length > 0, "download is valid gzip");
  console.log("ok  source UI archive download");

  const destination = await signedPage(b.base, b.token);
  const observer = await signedPage(b.base, b.token);
  await destination.getByRole("button", { name: "Import a project archive", exact: true }).click();
  await destination.getByLabel("Project archive (.tar.gz)", { exact: true }).setInputFiles(archivePath);
  await destination.getByRole("checkbox", { name: /I understand this imports/ }).check();
  let posts = 0;
  destination.on("request", (request) => {
    if (request.method() === "POST" && request.url().endsWith("/api/project-archives")) posts++;
  });
  const importResponse = destination.waitForResponse((response) => response.request().method() === "POST" && response.url().endsWith("/api/project-archives"));
  await destination.getByRole("button", { name: "Import as private project" }).click();
  const imported = await importResponse;
  assert.equal(imported.status(), 201, await imported.text());
  const result = await imported.json();
  await visible(destination, "ARC imported");
  const totalRows = Object.values(result.report.rows as Record<string, number>).reduce((sum, count) => sum + count, 0);
  await visible(destination, `${totalRows} records and 1 file imported.`);
  await visible(observer, "Archive specimen");
  assert.equal(posts, 1, "import is submitted once");
  await visible(destination, "unresolved reference");
  await visible(destination, "999999");
  assert(destination.url().endsWith("/projects/import"), "warnings stay on screen");
  await destination.setViewportSize({ width: 390, height: 844 });
  assert(await destination.evaluate(() => document.documentElement.scrollWidth <= innerWidth), "mobile report overflows");
  await destination.screenshot({ path: join(process.env.E2E_SCREENSHOT_DIR ?? tmpdir(), "lific-archive-import-mobile.png"), fullPage: true });
  const projects = await b.api("/projects");
  assert.equal(projects.length, 1);
  assert.equal(projects[0].is_public, false);
  assert.equal((await a.api("/projects"))[0].is_public, true, "source remains published and untouched");
  const relations = await b.api(`/projects/${projects[0].id}/relations`);
  assert.equal(relations.length, 1, "dependency edge transfers");
  await destination.getByRole("button", { name: "Open imported project" }).click();
  await visible(destination, "Archive specimen");
  await destination.goto(`${b.base}/ARC/issues/ARC-1`);
  await visible(destination, "Archive graph root");
  await visible(destination, "A comment carried across instances");
  await visible(destination, "archive-note.txt");
  await destination.getByText("archive-note.txt", { exact: true }).first().click();
  await visible(destination, "Archive attachment bytes");
  const attachmentDownload = destination.waitForEvent("download");
  await destination.getByRole("button", { name: "Download archive-note.txt", exact: true }).click();
  const attachment = await attachmentDownload;
  assert.equal(await Bun.file((await attachment.path())!).text(), "Archive attachment bytes");
  await destination.goto(`${b.base}/ARC/pages`);
  await destination.getByText("Archive notebook", { exact: true }).filter({ visible: true }).first().click();
  await destination.waitForURL(/\/ARC\/pages\/\d+$/);
  await visible(destination, "Archive notebook");
  await visible(destination, "A page carried across instances");
  console.log("ok  empty-instance import, counts, other-tab discovery, graph, comment, page, attachment, privacy, mobile report");

  await destination.goto(`${b.base}/projects/import`);
  await destination.getByLabel("Project archive (.tar.gz)", { exact: true }).waitFor();
  // A reload after success starts a fresh form; duplicate IDs must fail visibly.
  await destination.getByLabel("Project archive (.tar.gz)", { exact: true }).setInputFiles(archivePath);
  await destination.getByRole("checkbox", { name: /I understand this imports/ }).check();
  await destination.getByRole("button", { name: "Import as private project" }).click();
  await visible(destination, "already exists");
  assert.equal((await b.api("/projects")).length, 1);
  await destination.getByLabel("Project archive (.tar.gz)", { exact: true }).setInputFiles({ name: "not-an-archive.txt", mimeType: "text/plain", buffer: Buffer.from("bad") });
  await visible(destination, "ending in .tar.gz");
  assert(await destination.getByRole("button", { name: "Import as private project" }).isDisabled());
  // Synthetic File.size avoids allocating 128 MiB just to test browser validation.
  let invalidPosts = 0;
  destination.on("request", (request) => { if (request.method() === "POST" && request.url().endsWith("/api/project-archives")) invalidPosts++; });
  await destination.evaluate((max) => {
    const file = new File(["size-probe"], "too-large.tar.gz", { type: "application/gzip" });
    Object.defineProperty(file, "size", { value: max + 1 });
    const transfer = new DataTransfer(); transfer.items.add(file);
    const input = document.querySelector<HTMLInputElement>("#project-archive")!;
    input.files = transfer.files; input.dispatchEvent(new Event("change", { bubbles: true }));
  }, caps.max_upload_bytes);
  await visible(destination, "web upload limit");
  assert(await destination.getByRole("button", { name: "Import as private project" }).isDisabled());
  assert.equal(invalidPosts, 0);
  assert(await destination.evaluate(() => document.documentElement.scrollWidth <= innerWidth), "mobile form overflows");
  console.log("ok  duplicate ID, invalid extension, server-sourced browser size cap");

  await destination.getByLabel("Project archive (.tar.gz)", { exact: true }).setInputFiles(archivePath);
  await destination.getByRole("checkbox", { name: /I understand this imports/ }).check();
  let lostPosts = 0;
  let release!: () => void;
  const held = new Promise<void>((resolve) => { release = resolve; });
  await destination.route("**/api/project-archives", async (route) => {
    if (route.request().method() !== "POST") return route.continue();
    lostPosts++;
    await held;
    return route.abort("failed");
  });
  await destination.getByRole("button", { name: "Import as private project" }).click();
  await visible(destination, "Import in progress");
  assert(await destination.getByRole("button", { name: "Import in progress" }).isDisabled());
  await destination.getByRole("button", { name: "New project", exact: true }).first().click();
  await destination.getByRole("button", { name: "Import a project archive", exact: true }).click();
  await visible(destination, "Import in progress");
  release();
  await visible(destination, "The import may have completed");
  assert.equal(await destination.getByRole("button", { name: "Import as private project" }).count(), 0);
  await destination.reload();
  await visible(destination, "The import may have completed");
  assert.equal(lostPosts, 1);
  await destination.unroute("**/api/project-archives");
  console.log("ok  in-flight navigation and duplicate-submit guard, lost response remains unknown after reload, no automatic retry");

  const regularToken = await b.login("regular");
  const regular = await signedPage(b.base, regularToken, 390);
  await visible(regular, "New project");
  assert.equal(await regular.getByRole("button", { name: "Import a project archive", exact: true }).count(), 0);
  await regular.goto(`${b.base}/projects/import`);
  await visible(regular, "Only a signed-in instance admin");
  assert.equal(await regular.locator("#project-archive").count(), 0);
  const deniedForm = new FormData();
  deniedForm.append("archive", new File([await Bun.file(archivePath).bytes()], "ARC.lific.tar.gz", { type: "application/gzip" }));
  const denied = await fetch(`${b.base}/api/project-archives`, { method: "POST", headers: { Authorization: `Bearer ${regularToken}` }, body: deniedForm });
  assert.equal(denied.status, 403, "non-admin archive POST is refused in legacy mode");
  await regular.goto(`${b.base}/ARC/overview`);
  await visible(regular, "Archive specimen");
  assert.equal(await regular.getByRole("button", { name: "Download project archive" }).count(), 0);
  assert(await regular.evaluate(() => document.documentElement.scrollWidth <= innerWidth), "mobile overview overflows");
  await source.setViewportSize({ width: 390, height: 844 });
  await exportPanel.getByRole("button", { name: "Download project archive" }).waitFor();
  assert(await source.evaluate(() => document.documentElement.scrollWidth <= innerWidth), "mobile export overflows");

  await source.setViewportSize({ width: 1280, height: 900 });
  await a.api("/projects", "POST", { name: "Other project", identifier: "OTHER" });
  await staleDownload(source, a.base, async () => {
    await source.evaluate(() => {
      window.location.hash = "/OTHER/overview";
    });
    await visible(source, "Other project");
  });
  const sourceSecondToken = await a.login("second-admin");
  await staleDownload(source, a.base, () => changeSession(source, a.base, sourceSecondToken));
  const renewedSourceToken = await a.login("second-admin");
  await staleDownload(source, a.base, () => changeSession(source, a.base, renewedSourceToken));
  console.log("ok  delayed export aborts on project navigation, account change and same-user session replacement, no stale download event");

  await observer.context().close();
  const firstUser = await b.api("/auth/me");
  const secondToken = await b.login("second-admin");
  const secondUser = await (await fetch(`${b.base}/api/auth/me`, { headers: { Authorization: `Bearer ${secondToken}` } })).json();
  const firstKey = `lific:archive-import-pending:${firstUser.id}`;
  const secondKey = `lific:archive-import-pending:${secondUser.id}`;
  await visible(destination, "The import may have completed");
  await changeSession(destination, b.base, secondToken);
  await destination.locator("#project-archive").waitFor();
  assert.equal(await destination.getByText("The import may have completed", { exact: false }).count(), 0);
  assert.equal(await destination.evaluate((key) => sessionStorage.getItem(key), firstKey), "1");
  await destination.getByLabel("Project archive (.tar.gz)", { exact: true }).setInputFiles(archivePath);
  await destination.getByRole("checkbox", { name: /I understand this imports/ }).check();
  let releaseOld!: () => void, reachedOld!: () => void;
  const oldHeld = new Promise<void>((resolve) => { releaseOld = resolve; });
  const oldStarted = new Promise<void>((resolve) => { reachedOld = resolve; });
  await destination.route("**/api/project-archives", async (route) => {
    if (route.request().method() !== "POST") return route.continue();
    reachedOld();
    await oldHeld;
    await route.fulfill({ status: 201, json: { project: { id: 900, identifier: "PRIVATEOLD", is_public: false }, report: { project: "PRIVATEOLD", rows: { issues: 1 }, blobs: 0, external_references: ["private-old-account-reference"], external_reference_count: 1 } } });
  });
  await destination.getByRole("button", { name: "Import as private project" }).click();
  await oldStarted;
  await visible(destination, "Import in progress");
  await changeSession(destination, b.base, b.token);
  await visible(destination, "The import may have completed");
  releaseOld();
  await destination.waitForTimeout(250);
  assert.equal(await destination.getByText("PRIVATEOLD", { exact: false }).count(), 0);
  assert.equal(await destination.getByText("private-old-account-reference", { exact: false }).count(), 0);
  assert.equal(await destination.getByText("Import in progress", { exact: true }).count(), 0);
  await destination.unroute("**/api/project-archives");
  await destination.getByRole("button", { name: "I've checked the project list" }).click();
  assert.equal(await destination.evaluate((key) => sessionStorage.getItem(key), firstKey), null);
  assert.equal(await destination.evaluate((key) => sessionStorage.getItem(key), secondKey), "1");
  await changeSession(destination, b.base, secondToken);
  await visible(destination, "The import may have completed");
  const refreshed = await fetch(`${b.base}/api/auth/me/refresh`, { method: "POST", headers: { Authorization: `Bearer ${secondToken}`, "Content-Type": "application/json" }, body: JSON.stringify({ password: PASSWORD }) });
  assert.equal(refreshed.status, 200);
  const freshToken = (await refreshed.json()).token;
  assert.notEqual(freshToken, secondToken);
  await changeSession(destination, b.base, freshToken);
  await visible(destination, "The import may have completed");
  await destination.reload();
  await visible(destination, "The import may have completed");
  assert.equal(await destination.evaluate((key) => sessionStorage.getItem(key), secondKey), "1");
  await destination.evaluate(() => {
    localStorage.removeItem("lific_token");
    window.dispatchEvent(new Event("lific:session-change"));
  });
  await destination.getByText("Sign in to import a project archive.", { exact: true }).waitFor();
  assert.equal(await destination.getByText("The import may have completed", { exact: false }).count(), 0);
  assert.equal(await destination.evaluate((key) => sessionStorage.getItem(key), secondKey), "1");
  console.log("ok  pending warnings isolated by user ID, old-account progress/report hidden, own-only reset, session-refresh and logout retention");
  assert.equal(errors.length, 0, errors.join("\n"));
  console.log("ok  regular-user controls hidden and POST denied in legacy mode, mobile export, no unexpected console errors");
}

main().then(() => finish(0)).catch((error) => finish(1, String(error)));

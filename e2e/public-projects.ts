#!/usr/bin/env bun
/**
 * Published projects: browser feature smoke (LIF-465, LIF-471, LIF-583).
 *
 * Since LIF-583 there is no separate public view. A published project is read
 * at its ordinary links (`#/DEMO/issues/DEMO-1`, `/DEMO/pages/7`) by anyone,
 * in the normal app shell, through the ordinary `/api` routes, which answer a
 * request with no credential for published projects only. Signed out, every
 * way of changing something is hidden and a Sign in button stands in for it.
 *
 * The Rust tests prove the HTTP boundary: what the server will and will not
 * answer without a credential. They cannot prove the half of this feature
 * that only exists in a browser:
 *
 *   * that a published project's issue, page, plan and comment links open for
 *     a visitor with no session at all, with no sign-in step, and show no
 *     edit controls but a way to sign in;
 *   * that the same links on an unpublished project land on the login screen;
 *   * that old `/public/...` links (path and hash) end on the normal route;
 *   * that `/` opens the one published project, lists several, or logs in;
 *   * that a hostile issue body renders inert for a signed-out reader: no
 *     injected script, no `javascript:` link, no remote resource fetched;
 *   * that no response a signed-out browser receives carries an email
 *     address or a `lific_` credential;
 *   * that a member with a write role still gets the edit controls, and a
 *     signed-in non-member reads the project read-only;
 *   * that it is usable on a phone as well as a laptop;
 *   * that unpublishing closes the page a reader already had open.
 *
 * Everything this script starts, it owns and kills in `finally`: the server is
 * a child process of this script and dies with it, and the scratch directory
 * is removed. Nothing is left running in the background.
 *
 * Run locally:   devenv --profile e2e tasks run lific:e2e
 *                (or `cd e2e && bun public-projects.ts` after building)
 * Binary picked: $LIFIC_BIN, else target/debug/lific (a debug build reads
 * web/dist from disk at runtime; the task graph builds it first).
 */
import { chromium, type Browser, type BrowserContext, type Page } from "playwright";
import { spawn, execFileSync, type ChildProcess } from "node:child_process";
import { mkdtempSync, rmSync, existsSync, readFileSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { Database } from "bun:sqlite";

const ROOT = resolve(import.meta.dir, "..");
const BIN = process.env.LIFIC_BIN ?? join(ROOT, "target", "debug", "lific");
const PASSWORD = "public-smoke-password-123";

/**
 * A body written by somebody hostile, or by an agent that was talked into it.
 *
 * Every line is a vector the *regex* version of the renderer let through, kept
 * as one body so the browser assertions below read as a checklist. The three
 * families, in order:
 *
 *   1. script execution, the thing DOMPurify was always catching;
 *   2. **elements the regexes never looked at**, each of which makes the
 *      reader's browser fetch a URL the issue author chose: `video`, `audio`,
 *      `source`, `input type=image`, `object`, `embed`, `iframe`, `svg image`,
 *      `link rel=stylesheet`, `td background`, `style` blocks and inline
 *      `style="background:url()"`, `srcset`, `poster`;
 *   3. attribute syntax a regex could not parse: single-quoted and
 *      unquoted values, on the two tags a regex did look at.
 *
 * Everything here that reaches the network is a tracking pixel: it hands the
 * author the IP address, user agent and reading time of every visitor.
 */
const HOSTILE_BODY = [
  "# Hostile body",
  "",
  "## script",
  '<img src=x onerror="window.__pwned = true">',
  "<script>window.__pwned = true;</script>",
  '<svg onload="window.__pwned = true"></svg>',
  "[click me](javascript:window.__pwned=true)",
  "<a href='javascript:window.__pwned=true'>single quoted js</a>",
  "<a href=javascript:window.__pwned=true>unquoted js</a>",
  "",
  "## media and embeds the regex never saw",
  '<video src="https://tracker.invalid/v.mp4" poster="https://tracker.invalid/p.png"></video>',
  "<audio><source src=\"https://tracker.invalid/a.mp3\"></audio>",
  '<picture><source srcset="https://tracker.invalid/s.png"><img src="https://tracker.invalid/f.png"></picture>',
  '<input type="image" src="https://tracker.invalid/i.png">',
  '<object data="https://tracker.invalid/o.swf"></object>',
  '<embed src="https://tracker.invalid/e.swf">',
  '<iframe src="https://tracker.invalid/frame"></iframe>',
  '<svg><image href="https://tracker.invalid/svg.png"/></svg>',
  '<link rel="stylesheet" href="https://tracker.invalid/s.css">',
  "",
  "## css-shaped fetches",
  "<style>body { background: url(https://tracker.invalid/css.png); }</style>",
  '<p style="background-image:url(https://tracker.invalid/inline.png)">styled</p>',
  '<table><tr><td background="https://tracker.invalid/td.png">cell</td></tr></table>',
  '<img src="https://tracker.invalid/a.png" srcset="https://tracker.invalid/b.png 2x">',
  "",
  "## attribute syntax the regex could not parse",
  "<img src='https://tracker.invalid/single.png' alt='single quoted'>",
  "<img src=https://tracker.invalid/unquoted.png alt=unquoted>",
  // A single-quoted anchor to an ordinary site. `tracker.invalid` is reserved
  // in this body for things that must never be *fetched*; an outbound link is
  // allowed to survive (it is a destination the reader chooses), so it uses a
  // different host and is asserted on positively below.
  "<a href='https://example.com/single-quoted'>single quoted link</a>",
  "",
  "## attachments we are not entitled to",
  "[a file we are not allowed to see](/api/attachments/424242)",
  "![also not allowed](/api/attachments/424243)",
  "",
  "## things that must survive",
  "[a normal link](https://example.com/docs)",
  "[another project's issue](/PRIV/issues/PRIV-1)",
  // Prefix, not equality: DEMOX starts with DEMO and is a different project.
  "[a prefix-shaped project](/DEMOX/issues/DEMOX-1)",
].join("\n");

/** The counterpart: everything the renderer is supposed to keep. */
const FRIENDLY_BODY = [
  "# Friendly body",
  "",
  "Some **bold** and `inline code` and a [link](https://example.com/ok).",
  "",
  "- [x] a done task",
  "- [ ] a pending task",
  "",
  "| a | b |",
  "| - | - |",
  "| 1 | 2 |",
  "",
  "```",
  "a code block",
  "```",
  "",
  "> a quote",
  "",
  "A sibling issue: [see DEMO-1](/DEMO/issues/DEMO-1).",
  "",
  // A bare identifier in prose is auto-linked by the renderer, and a
  // signed-out reader can follow the generated route.
  "And bare in prose, DEMO-1 should auto-link.",
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

interface ApiResponse {
  url: string;
  status: number;
  text: string;
}

/** A page that records every request it makes, every API answer it gets,
 *  and every error it logs. */
async function watchedPage(context: BrowserContext, base: string) {
  const page = await context.newPage();
  const consoleErrors: string[] = [];
  const pageErrors: string[] = [];
  const offOrigin: string[] = [];
  const requests: string[] = [];
  const unauthorized: string[] = [];
  const limited: string[] = [];
  const pending: Promise<ApiResponse>[] = [];
  page.on("console", (msg) => {
    if (msg.type() === "error") consoleErrors.push(msg.text());
  });
  page.on("pageerror", (err) => pageErrors.push(String(err)));
  // Off-origin requests that a rendered issue body could plausibly have
  // caused. Deliberately not "everything off-origin": index.html loads the
  // app's webfonts from Google on every route, signed-in or not, which is an
  // app-wide choice that predates this feature and is not something an
  // issue body can influence. The kinds listed here are the ones a hostile
  // body CAN reach: an <img> tracking pixel, a fetch, a script, a media
  // element. A leak through the renderer still fails this test.
  const BODY_DRIVEN = new Set(["image", "xhr", "fetch", "script", "media"]);
  page.on("request", (req) => {
    const url = req.url();
    requests.push(url);
    if (!url.startsWith(base) && url.startsWith("http") && BODY_DRIVEN.has(req.resourceType())) {
      offOrigin.push(url);
    }
  });
  page.on("response", (res) => {
    const url = res.url();
    if (!url.startsWith(`${base}/api`)) return;
    // A 401 means the page asked for something that needs an account. A
    // signed-out reader's pages must not do that: the UI knows it has none.
    if (res.status() === 401) unauthorized.push(url);
    // The anonymous budgets (429 per IP, 503 instance-wide). A scenario that
    // trips one is measuring the budget, not the feature.
    if (res.status() === 429 || res.status() === 503) limited.push(`${res.status()} ${url}`);
    pending.push(
      res
        .text()
        .then((text) => ({ url, status: res.status(), text }))
        .catch(() => ({ url, status: res.status(), text: "" })),
    );
  });
  /** Every API answer this page received. Await before closing the context:
   *  bodies are gone once it closes. */
  const settle = () => Promise.all(pending);
  return { page, consoleErrors, pageErrors, offOrigin, requests, unauthorized, limited, settle };
}
type Watched = Awaited<ReturnType<typeof watchedPage>>;

async function textOf(page: Page): Promise<string> {
  return (await page.locator("body").innerText().catch(() => "")) ?? "";
}

async function waitForText(page: Page, text: string, timeout = 15_000): Promise<boolean> {
  return page
    .waitForFunction((t: string) => document.body.innerText.includes(t), text, { timeout })
    .then(() => true)
    .catch(() => false);
}

async function waitForHash(page: Page, prefix: string, timeout = 15_000): Promise<boolean> {
  return page
    .waitForFunction((p: string) => location.hash.startsWith(p), prefix, { timeout })
    .then(() => true)
    .catch(() => false);
}

/** Sign in the way a person does, through the login form. */
async function signIn(page: Page, base: string, username: string): Promise<void> {
  await page.goto(`${base}/#/login`, { waitUntil: "load", timeout: 15_000 });
  await page.fill("#login-identity", username);
  await page.fill("#login-password", PASSWORD);
  await page.click("button[type=submit]");
  const left = await page
    .waitForFunction(() => !location.hash.startsWith("#/login"), undefined, { timeout: 15_000 })
    .then(() => true)
    .catch(() => false);
  if (!left) throw new Error(`could not sign in as ${username}: ${(await textOf(page)).slice(0, 200)}`);
}

/** The rendered issue/page body: EditableMarkdown's read pane wrapping the
 *  shared Markdown component. (Comment bodies are `.prose` too, hence the
 *  `.em-rendered` qualifier.) */
const BODY_SELECTOR = ".em-rendered .prose";

async function main(): Promise<number> {
  if (!existsSync(BIN)) {
    console.error(`no binary at ${BIN}: run \`devenv --profile e2e tasks run lific:e2e\` first (or set LIFIC_BIN)`);
    return 1;
  }
  if (!existsSync(join(ROOT, "web", "dist", "index.html"))) {
    console.error("web/dist/index.html missing: run `devenv --profile e2e tasks run lific:e2e` first");
    return 1;
  }

  const scratch = mkdtempSync(join(tmpdir(), "lific-public-"));
  const config = join(scratch, "lific.toml");
  const db = join(scratch, "public.db");
  let server: ChildProcess | null = null;
  let browser: Browser | null = null;
  let serverLog = "";
  const failures: string[] = [];
  const fail = (scenario: string, message: string) =>
    failures.push(`${scenario}: ${message}`);
  // Every API answer any signed-out browser received, for the leak check.
  const signedOutResponses: ApiResponse[] = [];

  try {
    // ---- seed ----------------------------------------------------------
    cli(config, db, [
      "init", "--no-service", "--json",
      "--name", "Public Operator",
      "--auth-mode", "passwords",
      "--password", PASSWORD,
    ]);
    // Anonymous reads are budgeted per client address (240 a minute), and
    // this suite reads far more than one person would in a minute. Trust the
    // loopback proxy so each scenario can arrive from its own address (see
    // `run`), the way separate visitors would.
    const configText = readFileSync(config, "utf8");
    if (!/^trusted_proxies = \[\]$/m.test(configText)) {
      throw new Error("lific init no longer writes `trusted_proxies = []`; update the seed");
    }
    writeFileSync(config, configText.replace(/^trusted_proxies = \[\]$/m, 'trusted_proxies = ["127.0.0.1/32"]'));
    // Enforcement on, so a signed-in non-member really is below Viewer.
    cli(config, db, ["instance", "set", "--authz-enforced", "true", "--json"]);
    cli(config, db, ["user", "create", "-u", "writer", "-e", "writer@seed-leak.invalid", "-p", PASSWORD, "--json"]);
    cli(config, db, ["user", "create", "-u", "outsider", "-e", "outsider@seed-leak.invalid", "-p", PASSWORD, "--json"]);
    cli(config, db, ["project", "create", "--name", "Demo", "--identifier", "DEMO", "--json"]);
    cli(config, db, ["project", "create", "--name", "Secret", "--identifier", "PRIV", "--json"]);
    cli(config, db, ["project", "create", "--name", "Docs", "--identifier", "DOCS", "--json"]);
    cli(config, db, ["member", "add", "-p", "DEMO", "-u", "writer", "-r", "maintainer", "--json"]);
    cli(config, db, [
      "issue", "create", "--project", "DEMO",
      "--title", "Public issue one",
      "--description", "A body **anyone** may read.",
      "--json",
    ]);
    cli(config, db, ["issue", "create", "--project", "DEMO", "--title", "Public issue two", "--json"]);
    cli(config, db, [
      "issue", "create", "--project", "DEMO",
      "--title", "Hostile issue",
      "--description", HOSTILE_BODY,
      "--json",
    ]);
    cli(config, db, [
      "issue", "create", "--project", "DEMO",
      "--title", "Friendly issue",
      "--description", FRIENDLY_BODY,
      "--json",
    ]);
    cli(config, db, ["comment", "add", "DEMO-1", "--content", "A public comment body", "--json"]);
    cli(config, db, [
      "page", "create", "--project", "DEMO",
      "--title", "Public page",
      "--content", "Page body text",
      "--json",
    ]);
    cli(config, db, [
      "issue", "create", "--project", "PRIV",
      "--title", "Classified issue",
      "--description", "classified-marker-string",
      "--json",
    ]);
    cli(config, db, ["comment", "add", "PRIV-1", "--content", "classified comment", "--json"]);
    cli(config, db, [
      "page", "create", "--project", "PRIV",
      "--title", "Classified page",
      "--content", "classified-marker-string",
      "--json",
    ]);
    cli(config, db, ["issue", "create", "--project", "DOCS", "--title", "Docs issue", "--json"]);

    // Publishing and plans have no CLI by design, so the seed writes the rows
    // the same way smoke.ts seeds a project emoji.
    const seedDb = new Database(db);
    const projectId = (identifier: string) =>
      (seedDb.query("SELECT id FROM projects WHERE identifier = ?").get(identifier) as { id: number }).id;
    const demoId = projectId("DEMO");
    const privId = projectId("PRIV");
    seedDb.run("UPDATE projects SET is_public = 1 WHERE identifier = 'DEMO'");
    seedDb.run("INSERT INTO plans (project_id, sequence, title) VALUES (?, 1, 'Public plan')", [demoId]);
    seedDb.run("INSERT INTO plans (project_id, sequence, title) VALUES (?, 1, 'Classified plan')", [privId]);
    const one = <T>(sql: string, ...args: (string | number)[]) => seedDb.query(sql).get(...args) as T;
    const demoPage = one<{ id: number }>("SELECT id FROM pages WHERE title = 'Public page'").id;
    const privPage = one<{ id: number }>("SELECT id FROM pages WHERE title = 'Classified page'").id;
    const demoPlan = one<{ id: number }>("SELECT id FROM plans WHERE title = 'Public plan'").id;
    const privPlan = one<{ id: number }>("SELECT id FROM plans WHERE title = 'Classified plan'").id;
    const demoComment = one<{ id: number }>("SELECT id FROM comments WHERE content = 'A public comment body'").id;
    const privComment = one<{ id: number }>("SELECT id FROM comments WHERE content = 'classified comment'").id;
    const emails = (seedDb.query("SELECT email FROM users").all() as { email: string }[])
      .map((u) => u.email)
      .filter((e) => e && e.includes("@"));
    seedDb.close();
    const setPublic = (identifier: string, on: boolean) => {
      const live = new Database(db);
      live.run("UPDATE projects SET is_public = ? WHERE identifier = ?", [on ? 1 : 0, identifier]);
      live.close();
    };

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
    const desktop = { viewport: { width: 1280, height: 900 } };

    let visitor = 0;
    /** One scenario in a brand-new context: no cookie, no localStorage, and
     *  its own client address. */
    const run = async (
      name: string,
      options: { context?: Parameters<Browser["newContext"]>[0]; signedOut?: boolean },
      body: (w: Watched, context: BrowserContext) => Promise<void>,
    ) => {
      // ONLY=<part of a name> runs just the matching scenarios, for debugging.
      if (process.env.ONLY && !name.includes(process.env.ONLY)) return;
      visitor += 1;
      const context = await browser!.newContext(options.context ?? desktop);
      // Same-origin only: a third-party host (the webfonts) would reject the
      // extra header in its CORS preflight.
      const address = `198.51.100.${visitor}`;
      await context.route(`${base}/**`, (route) =>
        route.continue({ headers: { ...route.request().headers(), "x-forwarded-for": address } }),
      );
      const w = await watchedPage(context, base);
      try {
        await body(w, context);
      } catch (e) {
        fail(name, String(e));
      } finally {
        if (w.limited.length > 0) {
          fail(name, `hit an anonymous budget ${w.limited.length} times, first: ${w.limited[0]}`);
        }
        const responses = await w.settle();
        console.log(`     ${name}: ${responses.length} API responses`);
        if (options.signedOut !== false) signedOutResponses.push(...responses);
        await context.close();
      }
      console.log(`${failures.some((f) => f.startsWith(`${name}:`)) ? "FAIL" : "ok  "} ${name}`);
    };

    /** What every signed-out read of a published project must look like. */
    const expectReader = async (name: string, w: Watched, texts: string[]) => {
      for (const t of texts) {
        if (!(await waitForText(w.page, t))) fail(name, `expected visible text ${JSON.stringify(t)} not found`);
      }
      await w.page.waitForLoadState("networkidle", { timeout: 10_000 }).catch(() => {});
      const url = w.page.url();
      if (url.includes("#/login")) fail(name, `sent to the login page (${url})`);
      const body = await textOf(w.page);
      if (body.includes("Something went wrong")) fail(name, "rendered the error boundary fallback");
      if ((await w.page.locator("textarea").count()) > 0) {
        fail(name, "a text input was offered to a signed-out reader");
      }
      if ((await w.page.getByLabel("More create options").count()) > 0) {
        fail(name, "a create control was offered to a signed-out reader");
      }
      if ((await w.page.getByRole("button", { name: "Export" }).count()) > 0) {
        fail(name, "an export button was offered to a signed-out reader");
      }
      if ((await w.page.locator("[data-sign-in]:visible").count()) === 0) {
        fail(name, "no Sign in button was shown");
      }
      if (await w.page.evaluate(() => localStorage.getItem("lific_token"))) {
        fail(name, "a session token was minted for an anonymous visitor");
      }
      for (const u of w.unauthorized) fail(name, `asked for something that needs an account: ${u}`);
      for (const err of w.pageErrors) fail(name, `uncaught page error: ${err}`);
      for (const err of w.consoleErrors) fail(name, `console error: ${err}`);
    };

    // ---- 1. signed out: issue, page, plan and comment links ------------
    await run("signed-out issue link", {}, async (w) => {
      await w.page.goto(`${base}/#/DEMO/issues/DEMO-1`, { waitUntil: "load", timeout: 15_000 });
      await expectReader("signed-out issue link", w, [
        "Public issue one", "A public comment body", "Public Operator", "Read-only", "Sign in to comment",
      ]);
    });

    await run("signed-out page link", {}, async (w) => {
      await w.page.goto(`${base}/#/DEMO/pages/${demoPage}`, { waitUntil: "load", timeout: 15_000 });
      await expectReader("signed-out page link", w, ["Public page", "Page body text", "Read-only"]);
    });

    await run("signed-out plan link", {}, async (w) => {
      await w.page.goto(`${base}/#/DEMO/plans/${demoPlan}`, { waitUntil: "load", timeout: 15_000 });
      await expectReader("signed-out plan link", w, ["Public plan"]);
    });

    await run("signed-out comment link", {}, async (w) => {
      // Path style, the form "Copy link" on a comment produces.
      await w.page.goto(`${base}/DEMO/issues/DEMO-1#comment-${demoComment}`, {
        waitUntil: "load",
        timeout: 15_000,
      });
      await expectReader("signed-out comment link", w, ["Public issue one", "A public comment body"]);
    });

    await run("signed-out project views", {}, async (w) => {
      for (const [route, text] of [
        ["/DEMO/issues", "Public issue two"],
        ["/DEMO/board", "Public issue two"],
        ["/DEMO/pages", "Public page"],
        ["/DEMO/plans", "Public plan"],
        ["/DEMO/overview", "Demo"],
      ] as const) {
        await w.page.goto(`${base}/#${route}`, { waitUntil: "load", timeout: 15_000 });
        if (!(await waitForText(w.page, text))) fail("signed-out project views", `${route} did not render ${text}`);
        if (w.page.url().includes("#/login")) fail("signed-out project views", `${route} went to login`);
      }
      await expectReader("signed-out project views", w, []);
    });

    // ---- 2. signed out: an unpublished project is the login screen -----
    await run("unpublished links go to login", {}, async (w) => {
      const name = "unpublished links go to login";
      for (const link of [
        `${base}/#/PRIV/issues/PRIV-1`,
        `${base}/#/PRIV/pages/${privPage}`,
        `${base}/#/PRIV/plans/${privPlan}`,
        `${base}/PRIV/issues/PRIV-1#comment-${privComment}`,
        `${base}/#/NOPE/issues/NOPE-1`,
        `${base}/#/inbox`,
      ]) {
        await w.page.goto(link, { waitUntil: "load", timeout: 15_000 });
        if (!(await waitForHash(w.page, "#/login"))) {
          fail(name, `${link} did not land on login (${w.page.url()})`);
          continue;
        }
        if (!link.endsWith("/inbox") && !w.page.url().includes("next=")) {
          fail(name, `${link} forgot where it was going (${w.page.url()})`);
        }
        const body = await textOf(w.page);
        if (/classified/i.test(body)) fail(name, `${link} rendered an unpublished project's content`);
      }
      // An unpublished project and a missing one are the same answer, byte
      // for byte: the API is not an oracle for what the instance holds.
      const [priv, nope] = await w.page.evaluate(async (ids: number[]) => {
        const read = async (id: number) => {
          const r = await fetch(`/api/projects/${id}/index`, { credentials: "omit" });
          return { status: r.status, body: await r.text() };
        };
        return [await read(ids[0]), await read(ids[1])];
      }, [privId, 987654]);
      if (priv.status !== 404) fail(name, `an unpublished project's index answered ${priv.status}, not 404`);
      if (priv.status !== nope.status || priv.body !== nope.body) {
        fail(
          name,
          `an unpublished project is distinguishable from a missing one: ` +
            `${priv.status} ${JSON.stringify(priv.body)} vs ${nope.status} ${JSON.stringify(nope.body)}`,
        );
      }
    });

    // ---- 3. old public links end on the normal route -------------------
    await run("old public links redirect", {}, async (w) => {
      const name = "old public links redirect";
      for (const [link, want] of [
        [`${base}/public/DEMO/issues/DEMO-1`, "#/DEMO/issues/DEMO-1"],
        [`${base}/public/DEMO`, "#/DEMO/issues"],
        [`${base}/#/public/DEMO/issues/DEMO-1`, "#/DEMO/issues/DEMO-1"],
        [`${base}/#/public/DEMO`, "#/DEMO/issues"],
        [`${base}/#/public/DEMO/DEMO-2`, "#/DEMO/issues/DEMO-2"],
      ] as const) {
        await w.page.goto(link, { waitUntil: "load", timeout: 15_000 });
        await w.page.waitForFunction((h: string) => location.hash === h, want, { timeout: 10_000 }).catch(() => {});
        const got = new URL(w.page.url());
        if (got.hash !== want || got.pathname !== "/") {
          fail(name, `${link} ended on ${w.page.url()}, not ${base}/${want}`);
        }
      }
      // And an in-app navigation to an old hash, not just a cold load.
      await w.page.evaluate(() => {
        window.location.hash = "#/public/DEMO/pages";
      });
      await w.page.waitForFunction(() => location.hash === "#/DEMO/pages", undefined, { timeout: 10_000 })
        .catch(() => fail(name, `an in-app #/public link was not rewritten (${w.page.url()})`));
      if (!(await waitForText(w.page, "Public page"))) fail(name, "the rewritten route did not render");
    });

    // ---- 4. the root: one, several or no published projects ------------
    await run("root with one published project", {}, async (w) => {
      const name = "root with one published project";
      await w.page.goto(`${base}/`, { waitUntil: "load", timeout: 15_000 });
      if (!(await waitForHash(w.page, "#/DEMO/overview"))) {
        fail(name, `/ did not open the published project (${w.page.url()})`);
      }
      await expectReader(name, w, ["Demo"]);
    });

    await run("root with two published projects", {}, async (w) => {
      const name = "root with two published projects";
      setPublic("DOCS", true);
      try {
        await w.page.goto(`${base}/`, { waitUntil: "load", timeout: 15_000 });
        const listed = await w.page
          .locator("[data-published-projects] a")
          .nth(1)
          .waitFor({ timeout: 15_000 })
          .then(() => true)
          .catch(() => false);
        if (!listed) fail(name, `/ did not list the published projects (${w.page.url()})`);
        const links = await w.page.locator("[data-published-projects] a").count();
        if (links !== 2) fail(name, `listed ${links} projects, expected 2`);
        const body = await textOf(w.page);
        for (const t of ["Demo", "Docs"]) if (!body.includes(t)) fail(name, `${t} is not listed`);
        if (body.includes("Secret")) fail(name, "an unpublished project was listed");
        await w.page.locator("[data-published-projects] a", { hasText: "Docs" }).click();
        if (!(await waitForHash(w.page, "#/DOCS/overview"))) fail(name, `the Docs link went to ${w.page.url()}`);
        await expectReader(name, w, ["Docs"]);
      } finally {
        setPublic("DOCS", false);
      }
    });

    await run("root with nothing published", {}, async (w) => {
      const name = "root with nothing published";
      setPublic("DEMO", false);
      try {
        await w.page.goto(`${base}/`, { waitUntil: "load", timeout: 15_000 });
        if (!(await waitForHash(w.page, "#/login"))) fail(name, `/ did not show login (${w.page.url()})`);
      } finally {
        setPublic("DEMO", true);
      }
    });

    // ---- 5. hostile markdown, read signed out --------------------------
    await run("hostile markdown", { context: {} }, async (w) => {
      const scenario = "hostile markdown";
      await w.page.goto(`${base}/#/DEMO/issues/DEMO-3`, { waitUntil: "load", timeout: 15_000 });
      await w.page.locator(BODY_SELECTOR).first().waitFor({ timeout: 15_000 });
      await w.page.waitForLoadState("networkidle", { timeout: 10_000 }).catch(() => {});

      const pwned = await w.page.evaluate(
        () => (window as unknown as { __pwned?: boolean }).__pwned === true,
      );
      if (pwned) fail(scenario, "injected script executed");

      const markup = await w.page.evaluate((sel: string) => {
        const el = document.querySelector(sel);
        return el ? el.innerHTML : "";
      }, BODY_SELECTOR);
      if (markup === "") fail(scenario, "the issue body did not render at all");

      // Every attribute of every surviving element, and every tag name.
      // Asserting over these rather than over `innerHTML` is the difference
      // between "the string `javascript:` is nowhere on the page" and "no
      // attribute is a javascript: URL". A body that *talks about*
      // `javascript:` URLs is fine, it is inert text. What must not exist is
      // an attribute the browser would act on.
      //
      // Two regions are the *app's* output rather than the sanitizer's, and
      // are excluded so the checklist below stays about the author's markup:
      //
      //   * `span.attachment-view-host`: LIF-418 replaces each attachment
      //     chip with a mounted `AttachmentView`, whose Lucide icons are
      //     `<svg>`. Author markup never lands inside one of these.
      //   * `style` on `img[data-attachment-decorated]`: the same effect
      //     sets `cursor: zoom-in` on an attachment image *after*
      //     sanitization. The author's own `style=` attributes are on other
      //     elements and are still asserted on.
      const dom = await w.page.evaluate((sel: string) => {
        const root = document.querySelector(sel);
        const tags: string[] = [];
        const attrs: { name: string; value: string; tag: string }[] = [];
        for (const el of Array.from(root?.querySelectorAll("*") ?? [])) {
          if (el.closest(".attachment-view-host")) continue;
          const tag = el.tagName.toLowerCase();
          tags.push(tag);
          const appStyled = tag === "img" && el.hasAttribute("data-attachment-decorated");
          for (const a of Array.from(el.attributes)) {
            if (appStyled && a.name.toLowerCase() === "style") continue;
            attrs.push({ name: a.name.toLowerCase(), value: a.value, tag });
          }
        }
        return { tags, attrs };
      }, BODY_SELECTOR);

      // 1. Nothing executes, and no attribute is an executable URL.
      for (const [needle, what] of [
        [/onerror/i, "an onerror handler"],
        [/onload/i, "an onload handler"],
        [/<script/i, "a <script> tag"],
      ] as const) {
        if (needle.test(markup)) fail(scenario, `${what} survived sanitization`);
      }
      for (const attr of dom.attrs) {
        if (/^\s*javascript:/i.test(attr.value)) {
          fail(scenario, `a javascript: URL survived in <${attr.tag} ${attr.name}>`);
        }
        if (/^on/i.test(attr.name)) {
          fail(scenario, `an event handler survived: <${attr.tag} ${attr.name}>`);
        }
      }

      // 2. Nothing that can name a URL is left in the document at all.
      //    The list is exactly `PUBLIC_FORBID_TAGS` in web/src/lib/Markdown.svelte,
      //    which applies to every signed-out reader.
      for (const tag of [
        "video", "audio", "source", "track", "picture", "iframe", "object",
        "embed", "style", "link", "form", "svg", "image", "use", "math",
      ]) {
        if (dom.tags.includes(tag)) {
          fail(scenario, `a <${tag}> element survived into the signed-out body`);
        }
      }

      // 3. No URL-bearing attribute survived, whatever tag it was spelled
      //    on. `style` is on this list because `background:url()` is a
      //    fetch instruction wearing different clothes.
      for (const attr of dom.attrs) {
        if (["style", "srcset", "poster", "background", "ping"].includes(attr.name)) {
          fail(scenario, `a ${attr.name}= attribute survived on <${attr.tag}>`);
        }
      }

      // 4. The author's chosen host is in no attribute the browser acts on.
      //    (It may still appear as inert text, which is the correct outcome
      //    for a URL the renderer refused: the reader can see what was
      //    stripped.)
      for (const attr of dom.attrs) {
        if (/tracker\.invalid/i.test(attr.value)) {
          fail(scenario, `a remote resource URL survived in <${attr.tag} ${attr.name}>`);
        }
      }

      // 5. The things that SHOULD survive, so the test is not passing by
      //    stripping everything.
      if (!/example\.com\/docs/.test(markup)) {
        fail(scenario, "an ordinary outbound link was stripped; only unsafe ones should be");
      }
      if (!/example\.com\/single-quoted/.test(markup)) {
        fail(scenario, "a single-quoted outbound link was stripped");
      }
      if (!/Hostile body/.test(markup)) {
        fail(scenario, "the body's own prose was lost");
      }

      // 6. And the ground truth behind all of it: the browser fetched
      //    nothing off-origin.
      for (const url of w.offOrigin) fail(scenario, `fetched an off-origin resource: ${url}`);
      for (const err of w.pageErrors) fail(scenario, `uncaught page error: ${err}`);
      for (const u of w.unauthorized) fail(scenario, `asked for something that needs an account: ${u}`);
      // The two attachment ids nobody uploaded 404 (the thumbnail first,
      // then the full asset the error handler falls back to). That is the
      // correct outcome and the browser logs it; nothing else may be logged.
      for (const err of w.consoleErrors) {
        if (err.includes("Failed to load resource")) continue;
        fail(scenario, `console error: ${err}`);
      }
    });

    // ---- 5b. the friendly body keeps everything it should --------------
    // The sanitizer is an allowlist, and an allowlist's failure mode is
    // eating legitimate content. This is the other half of the pair.
    await run("safe markdown survives", { context: {} }, async (w) => {
      const scenario = "safe markdown survives";
      await w.page.goto(`${base}/#/DEMO/issues/DEMO-4`, { waitUntil: "load", timeout: 15_000 });
      await w.page.locator(BODY_SELECTOR).first().waitFor({ timeout: 15_000 });
      await w.page.waitForLoadState("networkidle", { timeout: 10_000 }).catch(() => {});
      const markup = await w.page.evaluate((sel: string) => {
        const el = document.querySelector(sel);
        return el ? el.innerHTML : "";
      }, BODY_SELECTOR);
      for (const [needle, what] of [
        [/<strong>/i, "bold"],
        [/<code>/i, "inline code"],
        [/<pre>/i, "a code block"],
        [/<table>/i, "a table"],
        [/<blockquote>/i, "a quote"],
        [/<ul>/i, "a list"],
        [/type="checkbox"/i, "a task-list checkbox"],
        [/example\.com\/ok/, "an outbound link"],
      ] as const) {
        if (!needle.test(markup)) fail(scenario, `${what} was stripped`);
      }
      // The generated links (auto-linked identifiers) are ordinary routes,
      // and a signed-out reader can follow them.
      const autoLink = w.page.locator(`${BODY_SELECTOR} a.identifier-link`).first();
      if ((await autoLink.count()) === 0) {
        fail(scenario, "a bare identifier in prose was not auto-linked");
      } else {
        const href = (await autoLink.getAttribute("href")) ?? "";
        if (!href.startsWith("#/DEMO/issues/")) {
          fail(scenario, `an auto-linked identifier is not a normal route: ${href}`);
        }
        await autoLink.click();
        if (!(await waitForText(w.page, "A public comment body", 10_000))) {
          fail(scenario, "the auto-linked identifier did not navigate");
        }
        if (!w.page.url().includes("#/DEMO/issues/DEMO-1")) {
          fail(scenario, `the auto-link went to ${w.page.url()}`);
        }
      }
      for (const err of w.pageErrors) fail(scenario, `uncaught page error: ${err}`);
      for (const err of w.consoleErrors) fail(scenario, `console error: ${err}`);
    });

    // ---- 5c. the whole project arrives in one list ----------------------
    await run("whole issue list arrives", {}, async (w) => {
      const scenario = "whole issue list arrives";
      // Comfortably past the old 100-row page size.
      const bulk = new Database(db);
      const next = bulk
        .query("SELECT COALESCE(MAX(sequence), 0) AS s FROM issues WHERE project_id = ?")
        .get(demoId) as { s: number };
      for (let i = 1; i <= 120; i += 1) {
        bulk.run(
          "INSERT INTO issues (project_id, sequence, title, description) VALUES (?, ?, ?, '')",
          [demoId, next.s + i, `Bulk issue ${i}`],
        );
      }
      bulk.close();

      await w.page.goto(`${base}/#/DEMO/issues`, { waitUntil: "load", timeout: 15_000 });
      // Default grouping is by status, and every seeded row is `backlog`,
      // so they all live in one group. Scoped to the content panel: the
      // sidebar's (collapsed) recent-issues list names the newest ones too.
      for (const title of ["Bulk issue 1", "Bulk issue 120"]) {
        const found = await w.page
          .locator("main")
          .getByText(title, { exact: true })
          .first()
          .waitFor({ timeout: 15_000 })
          .then(() => true)
          .catch(() => false);
        if (!found) {
          const attached = await w.page.locator("main").getByText(title, { exact: true }).count();
          fail(scenario, `${JSON.stringify(title)} never rendered (${attached} in the DOM)`);
        }
      }
      // Counted from the rows themselves rather than the text, so a
      // truncating list cannot pass by showing both ends and nothing else.
      const rows = await w.page.evaluate(
        () =>
          Array.from(document.querySelectorAll("[aria-label]")).filter((el) =>
            /^DEMO-\d+$/.test(el.getAttribute("aria-label") ?? ""),
          ).length,
      );
      if (rows < 100) {
        fail(scenario, `only ${rows} issue rows rendered; expected at least 100: ${(await textOf(w.page)).slice(0, 300)}`);
      }
      await expectReader(scenario, w, ["Public issue one"]);
    });

    // ---- 5d. navigating away mid-load does not leak content -------------
    await run("route race", { context: {} }, async (w) => {
      const scenario = "route race";
      await w.page.goto(`${base}/#/DEMO/issues`, { waitUntil: "load", timeout: 15_000 });
      await w.page.waitForLoadState("networkidle", { timeout: 10_000 }).catch(() => {});
      // Bounce between two issues fast enough that responses for an
      // abandoned one land after the next has rendered.
      for (let i = 0; i < 6; i += 1) {
        await w.page.evaluate(() => {
          window.location.hash = "#/DEMO/issues/DEMO-3";
        });
        await w.page.evaluate(() => {
          window.location.hash = "#/DEMO/issues";
        });
      }
      await w.page.evaluate(() => {
        window.location.hash = "#/DEMO/issues/DEMO-1";
      });
      if (!(await waitForText(w.page, "A public comment body", 10_000))) {
        fail(scenario, "the final route never rendered");
      }
      if ((await textOf(w.page)).includes("Hostile body")) {
        fail(scenario, "a stale route's content overwrote the current one");
      }
      for (const err of w.pageErrors) fail(scenario, `uncaught page error: ${err}`);
    });

    // ---- 6. phone ---------------------------------------------------------
    await run(
      "signed-out phone",
      { context: { viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true, deviceScaleFactor: 3 } },
      async (w) => {
        const scenario = "signed-out phone";
        await w.page.goto(`${base}/#/DEMO/issues/DEMO-1`, { waitUntil: "load", timeout: 15_000 });
        if (!(await waitForText(w.page, "Public issue one"))) {
          fail(scenario, "the issue title did not render at phone width");
        }
        await w.page.waitForLoadState("networkidle", { timeout: 10_000 }).catch(() => {});
        // Nothing may overflow the viewport horizontally on a phone.
        const overflow = await w.page.evaluate(
          () => document.documentElement.scrollWidth - document.documentElement.clientWidth,
        );
        if (overflow > 1) fail(scenario, `the page scrolls horizontally by ${overflow}px at 390px wide`);
        // The phone navigation: projects and a way in, no account rows.
        await w.page.getByLabel("Open navigation").first().tap();
        const nav = w.page.locator("[data-mobile-nav]");
        const signIn = await nav
          .locator("[data-sign-in]")
          .waitFor({ timeout: 10_000 })
          .then(() => true)
          .catch(() => false);
        if (!signIn) fail(scenario, "the phone navigation offers no Sign in");
        const navText = (await nav.innerText().catch(() => "")) ?? "";
        if (!navText.includes("Demo")) fail(scenario, "the phone navigation does not list the project");
        for (const t of ["Inbox", "Assigned to me", "Settings"]) {
          if (navText.includes(t)) fail(scenario, `the phone navigation offers ${t} to a signed-out reader`);
        }
        for (const u of w.unauthorized) fail(scenario, `asked for something that needs an account: ${u}`);
        for (const err of w.consoleErrors) fail(scenario, `console error: ${err}`);
        for (const err of w.pageErrors) fail(scenario, `uncaught page error: ${err}`);
      },
    );

    // ---- 7. unpublishing closes a page that is already open -------------
    await run("unpublish closes the project", { context: {} }, async (w) => {
      const scenario = "unpublish closes the project";
      await w.page.goto(`${base}/#/DEMO/issues/DEMO-1`, { waitUntil: "load", timeout: 15_000 });
      if (!(await waitForText(w.page, "Public issue one"))) {
        fail(scenario, "the issue did not render before unpublishing");
      }
      setPublic("DEMO", false);
      try {
        const status = await w.page.evaluate(async (id: number) => {
          const r = await fetch(`/api/projects/${id}/index`, { credentials: "omit" });
          return r.status;
        }, demoId);
        if (status !== 404) fail(scenario, `the index answered ${status} after unpublishing, not 404`);
        await w.page.reload({ waitUntil: "load", timeout: 15_000 });
        if (!(await waitForHash(w.page, "#/login"))) {
          fail(scenario, `reloading after unpublishing did not go to login (${w.page.url()})`);
        }
        if ((await textOf(w.page)).includes("Public issue one")) {
          fail(scenario, "the issue still rendered after publication was turned off");
        }
      } finally {
        setPublic("DEMO", true);
      }
      // Republishing brings the same link back.
      await w.page.goto(`${base}/#/DEMO/issues/DEMO-1`, { waitUntil: "load", timeout: 15_000 });
      if (!(await waitForText(w.page, "Public issue one"))) {
        fail(scenario, "republishing did not restore the original link");
      }
    });

    // ---- 8. a member with a write role still edits -------------------------
    await run("member can edit", { signedOut: false }, async (w) => {
      const scenario = "member can edit";
      await signIn(w.page, base, "writer");
      await w.page.evaluate(() => {
        window.location.hash = "#/DEMO/issues";
      });
      if (!(await waitForText(w.page, "Public issue one"))) fail(scenario, "the issue list did not render");
      const create = await w.page
        .getByLabel("More create options")
        .first()
        .waitFor({ timeout: 15_000 })
        .then(() => true)
        .catch(() => false);
      if (!create) fail(scenario, "a maintainer lost the create control");
      await w.page.evaluate(() => {
        window.location.hash = "#/DEMO/issues/DEMO-1";
      });
      if (!(await waitForText(w.page, "A public comment body"))) fail(scenario, "the issue did not render");
      if ((await w.page.locator("textarea").count()) === 0) {
        fail(scenario, "a maintainer was offered no comment composer");
      }
      if ((await w.page.locator("[data-sign-in]:visible").count()) > 0) {
        fail(scenario, "a signed-in member was asked to sign in");
      }
      for (const err of w.pageErrors) fail(scenario, `uncaught page error: ${err}`);
    });

    // ---- 9. a signed-in non-member reads read-only -----------------------
    await run("non-member reads read-only", { signedOut: false }, async (w) => {
      const scenario = "non-member reads read-only";
      await signIn(w.page, base, "outsider");
      await w.page.evaluate(() => {
        window.location.hash = "#/DEMO/issues";
      });
      if (!(await waitForText(w.page, "Public issue one"))) {
        fail(scenario, "a published project did not render for a non-member");
      }
      await w.page.waitForLoadState("networkidle", { timeout: 10_000 }).catch(() => {});
      if ((await w.page.getByLabel("More create options").count()) > 0) {
        fail(scenario, "a non-member was offered a create control");
      }
      await w.page.evaluate(() => {
        window.location.hash = "#/DEMO/issues/DEMO-1";
      });
      for (const t of ["A public comment body", "Read-only"]) {
        if (!(await waitForText(w.page, t))) fail(scenario, `expected visible text ${JSON.stringify(t)} not found`);
      }
      if ((await w.page.locator("textarea").count()) > 0) {
        fail(scenario, "a non-member was offered a text input");
      }
      if ((await w.page.locator("[data-sign-in]:visible").count()) > 0) {
        fail(scenario, "a signed-in reader was asked to sign in");
      }
      // An unpublished project stays closed to them.
      await w.page.evaluate(() => {
        window.location.hash = "#/PRIV/issues/PRIV-1";
      });
      await w.page.waitForLoadState("networkidle", { timeout: 10_000 }).catch(() => {});
      await new Promise((r) => setTimeout(r, 500));
      if ((await textOf(w.page)).includes("classified-marker-string")) {
        fail(scenario, "an unpublished project's content rendered for a non-member");
      }
      for (const err of w.pageErrors) fail(scenario, `uncaught page error: ${err}`);
    });

    // ---- 10. nothing private reached a signed-out browser ------------------
    {
      const scenario = "no account data reaches a signed-out reader";
      if (signedOutResponses.length === 0 && !process.env.ONLY) {
        fail(scenario, "no API responses were recorded");
      }
      for (const res of signedOutResponses) {
        for (const email of emails) {
          if (res.text.includes(email)) fail(scenario, `${res.url} carried the email address ${email}`);
        }
        if (res.text.includes("lific_")) fail(scenario, `${res.url} carried a lific_ credential`);
      }
      console.log(
        `${failures.some((f) => f.startsWith(`${scenario}:`)) ? "FAIL" : "ok  "} ${scenario} ` +
          `(${signedOutResponses.length} responses)`,
      );
    }
  } catch (e) {
    failures.push(`harness error: ${e instanceof Error ? (e.stack ?? e.message) : e}`);
  } finally {
    if (browser) await browser.close().catch(() => {});
    if (server && !server.killed) {
      server.kill("SIGTERM");
      await new Promise((r) => setTimeout(r, 500));
      if (server.exitCode === null) server.kill("SIGKILL");
    }
    rmSync(scratch, { recursive: true, force: true });
  }

  if (failures.length > 0) {
    console.error(
      `\npublic-projects smoke FAILED (${failures.length} problem${failures.length === 1 ? "" : "s"}):`,
    );
    for (const f of failures) console.error(`  - ${f}`);
    if (serverLog.trim()) {
      console.error("\nlast server output:");
      console.error(serverLog.split("\n").slice(-25).join("\n"));
    }
    return 1;
  }
  console.log("\npublic-projects smoke passed");
  return 0;
}

process.exit(await main());

#!/usr/bin/env bun
/**
 * Public project view: browser feature smoke (LIF-465).
 *
 * The Rust tests prove the HTTP boundary: what the server will and will not
 * answer. They cannot prove the half of this feature that only exists in a
 * browser:
 *
 *   * that `/public/DEMO` renders for a visitor with no session at all,
 *     instead of bouncing to /login or hanging on the bootstrap spinner;
 *   * that loading it fires no request at the private API: no
 *     `/api/instance`, no `/api/auth/me`, no auto-login, no realtime socket,
 *     which is the difference between "public page" and "page that happens to
 *     work when you are signed in";
 *   * that a hostile issue body renders inert: no injected script, no
 *     `javascript:` link, no remote image fetched, no probe at an attachment
 *     the reader is not entitled to;
 *   * that it is usable on a phone as well as a laptop;
 *   * that unpublishing closes the page a reader already had open.
 *
 * Everything this script starts, it owns and kills in `finally`: the server is
 * a child process of this script and dies with it, and the scratch directory
 * is removed. Nothing is left running in the background.
 *
 * Run locally:   bun install && bun public-projects.ts     (from e2e/)
 * Binary picked: $LIFIC_BIN, else target/debug/lific (a debug build reads
 * web/dist from disk at runtime, so run `bun run build` in web/ first).
 */
import { chromium, type Browser, type BrowserContext, type Route } from "playwright";
import { spawn, execFileSync, type ChildProcess } from "node:child_process";
import { mkdtempSync, rmSync, existsSync } from "node:fs";
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

/** Requests a public page is allowed to make: its own assets and its own API. */
function isPrivateRequest(url: string, base: string): boolean {
  if (!url.startsWith(base)) return true; // anything off-origin at all
  const path = url.slice(base.length);
  if (path.startsWith("/public/")) return false;
  if (path.startsWith("/assets/")) return false;
  if (path === "/" || path === "/favicon.ico" || path.endsWith(".png")) return false;
  return path.startsWith("/api") || path.startsWith("/oauth") || path.startsWith("/mcp");
}

/** Open a page that records every request it makes and every error it logs. */
async function watchedPage(context: BrowserContext, base: string) {
  const page = await context.newPage();
  const consoleErrors: string[] = [];
  const pageErrors: string[] = [];
  const privateRequests: string[] = [];
  const offOrigin: string[] = [];
  page.on("console", (msg) => {
    if (msg.type() === "error") consoleErrors.push(msg.text());
  });
  page.on("pageerror", (err) => pageErrors.push(String(err)));
  // Off-origin requests that a rendered issue body could plausibly have
  // caused. Deliberately not "everything off-origin": index.html loads the
  // app's webfonts from Google on every route, signed-in or not, which is an
  // app-wide choice that predates this feature and is not something a public
  // issue body can influence. The kinds listed here are the ones a hostile
  // body CAN reach: an <img> tracking pixel, a fetch, a script, a media
  // element. A leak through the renderer still fails this test.
  const BODY_DRIVEN = new Set(["image", "xhr", "fetch", "script", "media"]);
  page.on("request", (req) => {
    const url = req.url();
    if (!url.startsWith(base)) {
      if (url.startsWith("http") && BODY_DRIVEN.has(req.resourceType())) {
        offOrigin.push(url);
      }
      return;
    }
    if (isPrivateRequest(url, base)) privateRequests.push(url);
  });
  return { page, consoleErrors, pageErrors, privateRequests, offOrigin };
}

async function main(): Promise<number> {
  if (!existsSync(BIN)) {
    console.error(`no binary at ${BIN}: run \`cargo build\` first (or set LIFIC_BIN)`);
    return 1;
  }
  if (!existsSync(join(ROOT, "web", "dist", "index.html"))) {
    console.error("web/dist/index.html missing: run `bun run build` in web/ first");
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

  try {
    // ---- seed ----------------------------------------------------------
    cli(config, db, [
      "init", "--no-service", "--json",
      "--name", "Public Operator",
      "--auth-mode", "passwords",
      "--password", PASSWORD,
    ]);
    cli(config, db, ["project", "create", "--name", "Demo", "--identifier", "DEMO", "--json"]);
    cli(config, db, ["project", "create", "--name", "Secret", "--identifier", "PRIV", "--json"]);
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
      "issue", "create", "--project", "PRIV",
      "--title", "Classified issue",
      "--description", "classified-marker-string",
      "--json",
    ]);

    // Publishing is a UI/API action with no CLI flag by design, so the seed
    // sets the column the same way smoke.ts seeds a project emoji.
    const seedDb = new Database(db);
    seedDb.run("UPDATE projects SET is_public = 1 WHERE identifier = 'DEMO'");
    seedDb.close();

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

    browser = await chromium.launch();

    // ---- 1. desktop: the list, with no session -------------------------
    {
      const scenario = "public list (desktop)";
      // A brand-new context: no cookie, no localStorage, nothing signed in.
      const context = await browser.newContext({ viewport: { width: 1280, height: 900 } });
      const w = await watchedPage(context, base);
      try {
        await w.page.goto(`${base}/public/DEMO`, { waitUntil: "load", timeout: 15_000 });
        await w.page.waitForLoadState("networkidle", { timeout: 10_000 }).catch(() => {});
        const body = (await w.page.locator("body").innerText()) ?? "";

        if (w.page.url().includes("/login")) {
          fail(scenario, `redirected to the login page (${w.page.url()})`);
        }
        for (const expected of ["Public issue one", "Public issue two", "Public read-only view"]) {
          if (!body.includes(expected)) {
            fail(scenario, `expected visible text ${JSON.stringify(expected)} not found`);
          }
        }
        if (body.includes("Something went wrong")) {
          fail(scenario, "rendered the error boundary fallback");
        }
        // The whole point of the route: no private bootstrap ran.
        for (const url of w.privateRequests) {
          fail(scenario, `made a private-API request: ${url}`);
        }
        const token = await w.page.evaluate(() => localStorage.getItem("lific_token"));
        if (token) {
          fail(scenario, "a session token was minted for an anonymous visitor");
        }
        for (const err of w.consoleErrors) fail(scenario, `console error: ${err}`);
        for (const err of w.pageErrors) fail(scenario, `uncaught page error: ${err}`);
      } catch (e) {
        fail(scenario, String(e));
      } finally {
        await context.close();
      }
      console.log(`${failures.some((f) => f.startsWith(scenario)) ? "FAIL" : "ok  "} ${scenario}`);
    }

    // ---- 2. desktop: an issue, its body and its comment ----------------
    {
      const scenario = "public issue detail (desktop)";
      const context = await browser.newContext({ viewport: { width: 1280, height: 900 } });
      const w = await watchedPage(context, base);
      try {
        await w.page.goto(`${base}/public/DEMO`, { waitUntil: "load", timeout: 15_000 });
        await w.page.getByText("Public issue one").first().click();
        await w.page
          .waitForFunction(() => document.body.innerText.includes("A public comment body"), undefined, {
            timeout: 10_000,
          })
          .catch(() => {});
        const body = (await w.page.locator("body").innerText()) ?? "";
        for (const expected of ["Public issue one", "A public comment body", "DEMO-1"]) {
          if (!body.includes(expected)) {
            fail(scenario, `expected visible text ${JSON.stringify(expected)} not found`);
          }
        }
        for (const url of w.privateRequests) {
          fail(scenario, `made a private-API request: ${url}`);
        }
        for (const err of w.consoleErrors) fail(scenario, `console error: ${err}`);
        for (const err of w.pageErrors) fail(scenario, `uncaught page error: ${err}`);
      } catch (e) {
        fail(scenario, String(e));
      } finally {
        await context.close();
      }
      console.log(`${failures.some((f) => f.startsWith(scenario)) ? "FAIL" : "ok  "} ${scenario}`);
    }

    // ---- 3. mobile ------------------------------------------------------
    {
      const scenario = "public view (mobile)";
      const context = await browser.newContext({
        viewport: { width: 390, height: 844 },
        isMobile: true,
        hasTouch: true,
        deviceScaleFactor: 3,
      });
      const w = await watchedPage(context, base);
      try {
        await w.page.goto(`${base}/public/DEMO/DEMO-1`, { waitUntil: "load", timeout: 15_000 });
        await w.page.waitForLoadState("networkidle", { timeout: 10_000 }).catch(() => {});
        const body = (await w.page.locator("body").innerText()) ?? "";
        if (!body.includes("Public issue one")) {
          fail(scenario, "the issue title did not render at phone width");
        }
        // Nothing may overflow the viewport horizontally on a phone.
        const overflow = await w.page.evaluate(
          () => document.documentElement.scrollWidth - document.documentElement.clientWidth,
        );
        if (overflow > 1) {
          fail(scenario, `the page scrolls horizontally by ${overflow}px at 390px wide`);
        }
        for (const err of w.consoleErrors) fail(scenario, `console error: ${err}`);
        for (const err of w.pageErrors) fail(scenario, `uncaught page error: ${err}`);
      } catch (e) {
        fail(scenario, String(e));
      } finally {
        await context.close();
      }
      console.log(`${failures.some((f) => f.startsWith(scenario)) ? "FAIL" : "ok  "} ${scenario}`);
    }

    // ---- 4. hostile markdown -------------------------------------------
    {
      const scenario = "hostile markdown";
      const context = await browser.newContext();
      const w = await watchedPage(context, base);
      try {
        await w.page.goto(`${base}/public/DEMO/DEMO-3`, { waitUntil: "load", timeout: 15_000 });
        await w.page.waitForLoadState("networkidle", { timeout: 10_000 }).catch(() => {});

        const pwned = await w.page.evaluate(
          () => (window as unknown as { __pwned?: boolean }).__pwned === true,
        );
        if (pwned) fail(scenario, "injected script executed");

        const markup = await w.page.evaluate(() => {
          const el = document.querySelector(".public-md");
          return el ? el.innerHTML : "";
        });
        if (markup === "") fail(scenario, "the issue body did not render at all");

        // Every attribute of every surviving element, and every tag name.
        // Asserting over these rather than over `innerHTML` is the difference
        // between "the string `javascript:` is nowhere on the page" and "no
        // attribute is a javascript: URL". The first is not the property we
        // want: a body that *talks about* `javascript:` URLs, or shows one in
        // a code block, is fine, it is inert text. What must not exist is an
        // attribute the browser would act on.
        const dom = await w.page.evaluate(() => {
          const root = document.querySelector(".public-md");
          const tags: string[] = [];
          const attrs: { name: string; value: string; tag: string }[] = [];
          for (const el of Array.from(root?.querySelectorAll("*") ?? [])) {
            tags.push(el.tagName.toLowerCase());
            for (const a of Array.from(el.attributes)) {
              attrs.push({ name: a.name.toLowerCase(), value: a.value, tag: el.tagName.toLowerCase() });
            }
          }
          return { tags, attrs };
        });

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

        // 2. Nothing that can name a URL is left in the document at all. This
        //    is the assertion the regex renderer could not have passed: every
        //    one of these tags was invisible to it.
        for (const tag of [
          "video", "audio", "source", "iframe", "object", "embed", "svg",
          "link", "style", "input", "form", "button",
        ]) {
          if (dom.tags.includes(tag)) {
            fail(scenario, `a <${tag}> element survived into the public body`);
          }
        }

        // 3. No URL-bearing attribute survived, whatever tag it was spelled
        //    on. `style` is on this list because `background:url()` is a
        //    fetch instruction wearing different clothes.
        for (const attr of dom.attrs) {
          if (["style", "srcset", "poster", "background", "ping", "formaction", "data"].includes(attr.name)) {
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
          if (/\/api\/attachments\//.test(attr.value)) {
            fail(scenario, `an unauthorized attachment reference survived in <${attr.tag} ${attr.name}>`);
          }
          if (/PRIV\/issues|\/public\/PRIV/i.test(attr.value)) {
            fail(scenario, `a link into another project survived in <${attr.tag} ${attr.name}>`);
          }
          // A project whose identifier merely starts with this one is a
          // different project; a prefix match would have rewritten it.
          if (/DEMOX/i.test(attr.value)) {
            fail(scenario, `a prefix-shaped project link survived in <${attr.tag} ${attr.name}>`);
          }
        }

        // 5. The things that SHOULD survive, so the test is not passing by
        //    stripping everything.
        if (!/example\.com\/docs/.test(markup)) {
          fail(scenario, "an ordinary outbound link was stripped; only unsafe ones should be");
        }
        // The single-quoted anchor the old regex could not see: parsed
        // correctly now, and kept because it is an ordinary https link.
        if (!/example\.com\/single-quoted/.test(markup)) {
          fail(scenario, "a single-quoted outbound link was stripped");
        }
        // Every surviving outbound link is de-fanged.
        for (const attr of dom.attrs) {
          if (attr.tag === "a" && attr.name === "href" && /^https?:/i.test(attr.value)) {
            const rel = dom.attrs.find(
              (a) => a.tag === "a" && a.name === "rel" && a.value.includes("noreferrer"),
            );
            if (!rel) fail(scenario, `an outbound link kept its referrer: ${attr.value}`);
          }
        }
        if (!/Hostile body/.test(markup)) {
          fail(scenario, "the body's own prose was lost");
        }

        // 6. And the ground truth behind all of it: the browser fetched
        //    nothing off-origin and probed nothing private.
        for (const url of w.offOrigin) fail(scenario, `fetched an off-origin resource: ${url}`);
        for (const url of w.privateRequests) {
          fail(scenario, `made a private-API request: ${url}`);
        }
        for (const err of w.pageErrors) fail(scenario, `uncaught page error: ${err}`);
      } catch (e) {
        fail(scenario, String(e));
      } finally {
        await context.close();
      }
      console.log(`${failures.some((f) => f.startsWith(scenario)) ? "FAIL" : "ok  "} ${scenario}`);
    }

    // ---- 4b. the friendly body keeps everything it should ---------------
    // The sanitizer is an allowlist, and an allowlist's failure mode is
    // eating legitimate content. This is the other half of the pair.
    {
      const scenario = "safe markdown survives";
      const context = await browser.newContext();
      const w = await watchedPage(context, base);
      try {
        await w.page.goto(`${base}/public/DEMO/DEMO-4`, { waitUntil: "load", timeout: 15_000 });
        await w.page.waitForLoadState("networkidle", { timeout: 10_000 }).catch(() => {});
        const markup = await w.page.evaluate(() => {
          const el = document.querySelector(".public-md");
          return el ? el.innerHTML : "";
        });
        for (const [needle, what] of [
          [/<strong>/i, "bold"],
          [/<code>/i, "inline code"],
          [/<pre>/i, "a code block"],
          [/<table>/i, "a table"],
          [/<blockquote>/i, "a quote"],
          [/<ul>/i, "a list"],
          [/type="checkbox"/i, "a task-list checkbox"],
          [/example\.com\/ok/, "an outbound link"],
          [/#\/public\/DEMO\/DEMO-1/, "an in-project issue link"],
        ] as const) {
          if (!needle.test(markup)) fail(scenario, `${what} was stripped`);
        }
        // The in-project link must actually work.
        await w.page.getByText("see DEMO-1").first().click();
        await w.page
          .waitForFunction(() => document.body.innerText.includes("A public comment body"), undefined, {
            timeout: 10_000,
          })
          .catch(() => fail(scenario, "the in-project issue link did not navigate"));
        for (const err of w.pageErrors) fail(scenario, `uncaught page error: ${err}`);
        for (const err of w.consoleErrors) fail(scenario, `console error: ${err}`);
      } catch (e) {
        fail(scenario, String(e));
      } finally {
        await context.close();
      }
      console.log(`${failures.some((f) => f.startsWith(scenario)) ? "FAIL" : "ok  "} ${scenario}`);
    }

    // ---- 4c. the list pages, and the whole project arrives --------------
    {
      const scenario = "issue list pagination";
      const context = await browser.newContext();
      const w = await watchedPage(context, base);
      try {
        // One page is 100 server-side; seed past it so the walk is real.
        const bulk = new Database(db);
        const projectId = bulk
          .query("SELECT id FROM projects WHERE identifier = 'DEMO'")
          .get() as { id: number };
        const next = bulk
          .query("SELECT COALESCE(MAX(sequence), 0) AS s FROM issues WHERE project_id = ?")
          .get(projectId.id) as { s: number };
        for (let i = 1; i <= 120; i += 1) {
          bulk.run(
            "INSERT INTO issues (project_id, sequence, title, description) VALUES (?, ?, ?, '')",
            [projectId.id, next.s + i, `Bulk issue ${i}`],
          );
        }
        bulk.close();

        await w.page.goto(`${base}/public/DEMO`, { waitUntil: "load", timeout: 15_000 });
        // The last-seeded issue only appears once the second page lands.
        const walked = await w.page
          .waitForFunction(() => document.body.innerText.includes("Bulk issue 120"), undefined, {
            timeout: 15_000,
          })
          .then(() => true)
          .catch(() => false);
        if (!walked) {
          fail(scenario, "the view did not walk past the first page of issues");
        }
        // And the first page was on screen before the walk finished, i.e. the
        // list is not blocked on the whole project loading.
        const body = (await w.page.locator("body").innerText()) ?? "";
        if (!body.includes("Public issue one")) {
          fail(scenario, "the first page's issues are missing after the walk");
        }
        for (const err of w.consoleErrors) fail(scenario, `console error: ${err}`);
        for (const err of w.pageErrors) fail(scenario, `uncaught page error: ${err}`);
      } catch (e) {
        fail(scenario, String(e));
      } finally {
        await context.close();
      }
      console.log(`${failures.some((f) => f.startsWith(scenario)) ? "FAIL" : "ok  "} ${scenario}`);
    }

    // ---- 4d. comments page, and the rest is reachable ------------------
    {
      const scenario = "comment paging";
      const context = await browser.newContext();
      const w = await watchedPage(context, base);
      try {
        const bulk = new Database(db);
        const issue = bulk
          .query("SELECT id FROM issues WHERE title = 'Public issue one'")
          .get() as { id: number };
        for (let i = 1; i <= 60; i += 1) {
          bulk.run(
            "INSERT INTO comments (issue_id, user_id, content) VALUES (?, 1, ?)",
            [issue.id, `paged comment ${i}`],
          );
        }
        bulk.close();

        await w.page.goto(`${base}/public/DEMO/DEMO-1`, { waitUntil: "load", timeout: 15_000 });
        await w.page.waitForLoadState("networkidle", { timeout: 10_000 }).catch(() => {});

        let body = (await w.page.locator("body").innerText()) ?? "";
        if (!body.includes("61 comments")) {
          fail(scenario, "the total comment count was not shown");
        }
        if (body.includes("paged comment 60")) {
          fail(scenario, "the whole thread loaded at once; it should be paged");
        }
        // Truncation must be actionable, not an apology.
        const more = w.page.getByRole("button", { name: /Load more/ });
        if ((await more.count()) === 0) {
          fail(scenario, "no way to read the rest of the thread");
        } else {
          await more.first().click();
          const arrived = await w.page
            .waitForFunction(() => document.body.innerText.includes("paged comment 60"), undefined, {
              timeout: 10_000,
            })
            .then(() => true)
            .catch(() => false);
          if (!arrived) fail(scenario, "Load more did not fetch the next page");
          body = (await w.page.locator("body").innerText()) ?? "";
          if (!body.includes("A public comment body")) {
            fail(scenario, "the first page was dropped when the next one loaded");
          }
        }
        for (const url of w.privateRequests) fail(scenario, `private-API request: ${url}`);
        for (const err of w.consoleErrors) fail(scenario, `console error: ${err}`);
        for (const err of w.pageErrors) fail(scenario, `uncaught page error: ${err}`);
      } catch (e) {
        fail(scenario, String(e));
      } finally {
        await context.close();
      }
      console.log(`${failures.some((f) => f.startsWith(scenario)) ? "FAIL" : "ok  "} ${scenario}`);
    }

    // ---- 4e. navigating away mid-load does not leak content -------------
    {
      const scenario = "route race";
      const context = await browser.newContext();
      const w = await watchedPage(context, base);
      try {
        await w.page.goto(`${base}/public/DEMO`, { waitUntil: "load", timeout: 15_000 });
        await w.page.waitForLoadState("networkidle", { timeout: 10_000 }).catch(() => {});
        // Bounce between routes fast enough that responses for an abandoned
        // one land after the next has rendered.
        for (let i = 0; i < 6; i += 1) {
          await w.page.evaluate(() => {
            window.location.hash = "#/public/DEMO/DEMO-3";
          });
          await w.page.evaluate(() => {
            window.location.hash = "#/public/DEMO";
          });
        }
        await w.page.evaluate(() => {
          window.location.hash = "#/public/DEMO/DEMO-1";
        });
        const settled = await w.page
          .waitForFunction(() => document.body.innerText.includes("A public comment body"), undefined, {
            timeout: 10_000,
          })
          .then(() => true)
          .catch(() => false);
        if (!settled) fail(scenario, "the final route never rendered");
        const body = (await w.page.locator("body").innerText()) ?? "";
        if (body.includes("Hostile body")) {
          fail(scenario, "a stale route's content overwrote the current one");
        }
        for (const err of w.pageErrors) fail(scenario, `uncaught page error: ${err}`);
      } catch (e) {
        fail(scenario, String(e));
      } finally {
        await context.close();
      }
      console.log(`${failures.some((f) => f.startsWith(scenario)) ? "FAIL" : "ok  "} ${scenario}`);
    }

    // A held load-more response must not leave the next route busy or append stale comments.
    {
      const scenario = "load-more navigation race";
      const context = await browser.newContext();
      const w = await watchedPage(context, base);
      try {
        w.page.setDefaultTimeout(10_000);
        // Let the old response arrive after cancellation to exercise the generation guard.
        await w.page.addInitScript(() => {
          const original = window.fetch.bind(window);
          window.fetch = (input, init) => {
            const url = new URL(String(input), location.href);
            if (url.pathname.endsWith("/DEMO-1/comments") && url.searchParams.get("offset") === "1") {
              return original(input, { ...init, signal: undefined });
            }
            return original(input, init);
          };
        });
        const oldRequest = Promise.withResolvers<Route>();
        const newRequest = Promise.withResolvers<Route>();
        const comments = (id: number, content: string, hasMore: boolean) => ({
          comments: [{ id, content, created_at: "2026-09-07T12:00:00Z",
            updated_at: "2026-09-07T12:00:00Z", attachments: [] }],
          total: 2, limit: 50, offset: hasMore ? 0 : 1, has_more: hasMore,
        });
        await w.page.route("**/public/api/projects/DEMO/issues/*/comments?*", async (route) => {
          const url = new URL(route.request().url());
          const old = url.pathname.endsWith("/DEMO-1/comments");
          if (url.searchParams.get("offset") === "1") {
            (old ? oldRequest : newRequest).resolve(route);
          } else {
            await route.fulfill({ json: comments(old ? 101 : 201, old ? "Old first page" : "New first page", true) });
          }
        });
        const heldRoute = (promise: Promise<Route>) => Promise.race([
          promise,
          Bun.sleep(10_000).then(() => { throw new Error("load-more request never arrived"); }),
        ]);

        await w.page.goto(`${base}/public/DEMO/DEMO-1`, { waitUntil: "load" });
        await w.page.getByRole("button", { name: /Load more/ }).click();
        const oldRoute = await heldRoute(oldRequest.promise);
        if (!(await w.page.getByRole("button", { name: "Loading…", exact: true }).isDisabled())) {
          fail(scenario, "the old request did not enter its busy state");
        }
        await w.page.evaluate(() => { window.location.hash = "#/public/DEMO/DEMO-2"; });
        await w.page.getByText("New first page", { exact: true }).waitFor();
        await w.page.getByRole("button", { name: /Load more/ }).click();
        const newRoute = await heldRoute(newRequest.promise);

        const oldResponse = w.page.waitForResponse((response) => response.url() === oldRoute.request().url());
        await oldRoute.fulfill({ json: comments(102, "Stale old page must not render", false) });
        await (await oldResponse).finished();
        await w.page.evaluate(() => new Promise<void>((resolve) => {
          requestAnimationFrame(() => requestAnimationFrame(() => resolve()));
        }));
        if (!(await w.page.getByRole("button", { name: "Loading…", exact: true }).isDisabled())) {
          fail(scenario, "the old request cleared the new request's busy state");
        }
        if ((await w.page.locator("body").innerText()).includes("Stale old page")) {
          fail(scenario, "the old request appended comments to the new issue");
        }
        await newRoute.fulfill({ json: comments(202, "New second page", false) });
        await w.page.getByText("New second page", { exact: true }).waitFor();
        if (!(await w.page.getByText("New first page", { exact: true }).isVisible())) {
          fail(scenario, "the new issue lost its first page");
        }
        if (await w.page.getByRole("button", { name: /Load more|Loading…/ }).count()) {
          fail(scenario, "the completed thread retained a load-more button");
        }
        for (const url of w.privateRequests) fail(scenario, `private-API request: ${url}`);
        for (const err of w.pageErrors) fail(scenario, `uncaught page error: ${err}`);
      } catch (e) {
        fail(scenario, String(e));
      } finally {
        await context.close();
      }
      console.log(`${failures.some((f) => f.startsWith(scenario)) ? "FAIL" : "ok  "} ${scenario}`);
    }

    // ---- 5. a private project is not reachable --------------------------
    {
      const scenario = "private project stays private";
      const context = await browser.newContext();
      const w = await watchedPage(context, base);
      try {
        await w.page.goto(`${base}/public/PRIV`, { waitUntil: "load", timeout: 15_000 });
        await w.page.waitForLoadState("networkidle", { timeout: 10_000 }).catch(() => {});
        const body = (await w.page.locator("body").innerText()) ?? "";
        if (body.includes("classified-marker-string") || body.includes("Classified issue")) {
          fail(scenario, "an unpublished project's content rendered");
        }
        if (!body.includes("isn't published")) {
          fail(scenario, `expected the "not published" message, got: ${body.slice(0, 200)}`);
        }
      } catch (e) {
        fail(scenario, String(e));
      } finally {
        await context.close();
      }
      console.log(`${failures.some((f) => f.startsWith(scenario)) ? "FAIL" : "ok  "} ${scenario}`);
    }

    // ---- 6. unpublishing closes a page that is already open -------------
    {
      const scenario = "unpublish closes the view";
      const context = await browser.newContext();
      const w = await watchedPage(context, base);
      try {
        await w.page.goto(`${base}/public/DEMO/DEMO-1`, { waitUntil: "load", timeout: 15_000 });
        await w.page.waitForLoadState("networkidle", { timeout: 10_000 }).catch(() => {});
        if (!(await w.page.locator("body").innerText()).includes("Public issue one")) {
          fail(scenario, "the issue did not render before unpublishing");
        }

        const live = new Database(db);
        live.run("UPDATE projects SET is_public = 0 WHERE identifier = 'DEMO'");
        live.close();

        // A direct fetch proves the server closed it; the reload proves the
        // page a reader already had open cannot be refreshed back into life.
        const status = await w.page.evaluate(async (b: string) => {
          const r = await fetch(`${b}/public/api/projects/DEMO/issues/DEMO-1`, {
            credentials: "omit",
          });
          return r.status;
        }, base);
        if (status !== 404) {
          fail(scenario, `the detail endpoint answered ${status} after unpublishing, not 404`);
        }

        await w.page.reload({ waitUntil: "load", timeout: 15_000 });
        await w.page.waitForLoadState("networkidle", { timeout: 10_000 }).catch(() => {});
        const body = (await w.page.locator("body").innerText()) ?? "";
        if (body.includes("Public issue one")) {
          fail(scenario, "the issue still rendered after publication was turned off");
        }

        // Republishing restores the same address, no new link.
        const back = new Database(db);
        back.run("UPDATE projects SET is_public = 1 WHERE identifier = 'DEMO'");
        back.close();
        await w.page.reload({ waitUntil: "load", timeout: 15_000 });
        await w.page
          .waitForFunction(() => document.body.innerText.includes("Public issue one"), undefined, {
            timeout: 10_000,
          })
          .catch(() =>
            fail(scenario, "republishing did not restore the original address"),
          );
      } catch (e) {
        fail(scenario, String(e));
      } finally {
        await context.close();
      }
      console.log(`${failures.some((f) => f.startsWith(scenario)) ? "FAIL" : "ok  "} ${scenario}`);
    }

    // ---- 7. the signed-in app still works -------------------------------
    // Adding a login-free route must not have loosened, or broken, the route
    // that does need a login.
    {
      const scenario = "signed-in app is unaffected";
      const context = await browser.newContext();
      const page = await context.newPage();
      try {
        await page.goto(`${base}/DEMO/issues`, { waitUntil: "load", timeout: 15_000 });
        await page.waitForLoadState("networkidle", { timeout: 10_000 }).catch(() => {});
        if (!page.url().includes("/login")) {
          fail(scenario, `an anonymous visitor reached ${page.url()} instead of the login page`);
        }
      } catch (e) {
        fail(scenario, String(e));
      } finally {
        await context.close();
      }
      console.log(`${failures.some((f) => f.startsWith(scenario)) ? "FAIL" : "ok  "} ${scenario}`);
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

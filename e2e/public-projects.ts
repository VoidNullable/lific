#!/usr/bin/env bun
/**
 * Public project view: browser feature smoke (LIF-465, redesigned in LIF-471).
 *
 * Since LIF-471 there is no bespoke public component: `#/public/DEMO`
 * redirects to `#/public/DEMO/issues` and the ordinary IssueList /
 * IssueDetail / PageList / PageDetail render inside `PublicLayout`, with
 * every request rewritten onto `/public/api/...` and no credential attached.
 * So these assertions are about the *real* app running in public scope.
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
import { chromium, type Browser, type BrowserContext } from "playwright";
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
  "",
  // LIF-471: a bare identifier in prose is auto-linked by the renderer, and
  // in public scope that generated route must stay under /public.
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
  const requests: string[] = [];
  page.on("request", (req) => {
    const url = req.url();
    requests.push(url);
    if (!url.startsWith(base)) {
      if (url.startsWith("http") && BODY_DRIVEN.has(req.resourceType())) {
        offOrigin.push(url);
      }
      return;
    }
    if (isPrivateRequest(url, base)) privateRequests.push(url);
  });
  return { page, consoleErrors, pageErrors, privateRequests, offOrigin, requests };
}

/** The rendered issue/page body: EditableMarkdown's read pane wrapping the
 *  shared Markdown component. (Comment bodies are `.prose` too, hence the
 *  `.em-rendered` qualifier.) */
const BODY_SELECTOR = ".em-rendered .prose";

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
        // LIF-471: the bare project route canonicalizes onto the issue list.
        if (!w.page.url().includes("#/public/DEMO/issues")) {
          fail(scenario, `/public/DEMO did not redirect to its issue list (${w.page.url()})`);
        }
        // The shell: one project, its two sections, the public tag, a way in.
        for (const expected of [
          "Public issue one", "Public issue two", "Issues", "Pages", "public", "Sign in",
        ]) {
          if (!body.includes(expected)) {
            fail(scenario, `expected visible text ${JSON.stringify(expected)} not found`);
          }
        }
        // The LIF-465 banner is gone; the shell says "public" instead.
        if (body.includes("Public read-only view")) {
          fail(scenario, "the retired 'Public read-only view' banner is still rendered");
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
        await w.page.goto(`${base}/public/DEMO/issues`, { waitUntil: "load", timeout: 15_000 });
        await w.page.getByText("Public issue one").first().click();
        await w.page
          .waitForFunction(() => document.body.innerText.includes("A public comment body"), undefined, {
            timeout: 10_000,
          })
          .catch(() => {});
        if (!w.page.url().includes("#/public/DEMO/issues/DEMO-1")) {
          fail(scenario, `clicking the row landed on ${w.page.url()}`);
        }
        const body = (await w.page.locator("body").innerText()) ?? "";
        for (const expected of [
          "Public issue one",
          "A public comment body",
          "DEMO-1",
          // The commenter's display name, and nothing else about them.
          "Public Operator",
          // LIF-471: the read-only cue the ordinary detail view already had.
          "Read-only",
        ]) {
          if (!body.includes(expected)) {
            fail(scenario, `expected visible text ${JSON.stringify(expected)} not found`);
          }
        }
        // Nothing to write with: no comment composer, no body editor.
        if ((await w.page.locator("textarea").count()) > 0) {
          fail(scenario, "a text input was offered on a read-only public page");
        }
        // And no export, which is a signed-in affordance.
        if ((await w.page.getByRole("button", { name: "Export" }).count()) > 0) {
          fail(scenario, "the export button was offered on a public page");
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
        // Hash form. A detail deep link in *path* form
        // (`/public/DEMO/issues/DEMO-1`) is currently mis-parsed by
        // `splitResourcePath` in web/src/lib/commentLinks.ts, which reads
        // `/public` as an app base path and routes to the private
        // `/DEMO/issues/DEMO-1`. Reported separately; the hash form is what
        // the app canonicalizes every in-app URL to, so it is what a reader
        // copies out of the address bar.
        await w.page.goto(`${base}/#/public/DEMO/issues/DEMO-1`, {
          waitUntil: "load",
          timeout: 15_000,
        });
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
        await w.page.goto(`${base}/#/public/DEMO/issues/DEMO-3`, { waitUntil: "load", timeout: 15_000 });
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
        // attribute is a javascript: URL". The first is not the property we
        // want: a body that *talks about* `javascript:` URLs, or shows one in
        // a code block, is fine, it is inert text. What must not exist is an
        // attribute the browser would act on.
        //
        // Two regions are the *app's* output rather than the sanitizer's, and
        // are excluded so the checklist below stays about the author's markup:
        //
        //   * `span.attachment-view-host` — LIF-418 replaces each attachment
        //     chip with a mounted `AttachmentView`, whose Lucide icons are
        //     `<svg>`. Author markup never lands inside one of these.
        //   * `style` on `img[data-attachment-decorated]` — the same effect
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

        // 2. Nothing that can name a URL is left in the document at all. This
        //    is the assertion the regex renderer could not have passed: every
        //    one of these tags was invisible to it.
        //    The list is exactly `PUBLIC_FORBID_TAGS` in web/src/lib/Markdown.svelte.
        for (const tag of [
          "video", "audio", "source", "track", "picture", "iframe", "object",
          "embed", "style", "link", "form", "svg", "image", "use", "math",
        ]) {
          if (dom.tags.includes(tag)) {
            fail(scenario, `a <${tag}> element survived into the public body`);
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
        //
        //    An attachment the reader is not entitled to is NOT asserted on
        //    here any more (LIF-471): the chip becomes a mounted
        //    AttachmentView and the image is rewritten onto the public
        //    thumbnail route, so the surviving markup legitimately names
        //    `/public/api/.../attachments/...`. What matters is the request
        //    it makes, asserted below. Cross-project links are ordinary
        //    relative hrefs now and survive on purpose.
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
        // The single-quoted anchor the old regex could not see: parsed
        // correctly now, and kept because it is an ordinary https link.
        if (!/example\.com\/single-quoted/.test(markup)) {
          fail(scenario, "a single-quoted outbound link was stripped");
        }
        if (!/Hostile body/.test(markup)) {
          fail(scenario, "the body's own prose was lost");
        }

        // 6. And the ground truth behind all of it: the browser fetched
        //    nothing off-origin and probed nothing private. In particular no
        //    request reached the credentialed `/api/attachments/...` route
        //    for the two attachment ids this reader is not entitled to; the
        //    404s all land on the anonymous mirror.
        for (const url of w.offOrigin) fail(scenario, `fetched an off-origin resource: ${url}`);
        for (const url of w.privateRequests) {
          fail(scenario, `made a private-API request: ${url}`);
        }
        for (const url of w.requests) {
          if (url.startsWith(`${base}/api/attachments/`)) {
            fail(scenario, `probed a private attachment route: ${url}`);
          }
        }
        for (const err of w.pageErrors) fail(scenario, `uncaught page error: ${err}`);
        // The two unauthorized attachment ids 404 on the public mirror (the
        // thumbnail first, then the full asset the error handler falls back
        // to). That is the correct outcome and the browser logs it; nothing
        // else may be logged.
        for (const err of w.consoleErrors) {
          if (err.includes("Failed to load resource")) continue;
          fail(scenario, `console error: ${err}`);
        }
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
        await w.page.goto(`${base}/#/public/DEMO/issues/DEMO-4`, { waitUntil: "load", timeout: 15_000 });
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
        // LIF-471: the generated links (auto-linked identifiers) are the ones
        // the renderer owns, and they must stay inside the public view. An
        // authored relative link is the author's, and is left alone.
        const autoLink = w.page.locator(`${BODY_SELECTOR} a.identifier-link`).first();
        if ((await autoLink.count()) === 0) {
          fail(scenario, "a bare identifier in prose was not auto-linked");
        } else {
          const href = (await autoLink.getAttribute("href")) ?? "";
          if (!href.startsWith("#/public/DEMO/issues/")) {
            fail(scenario, `an auto-linked identifier escaped the public view: ${href}`);
          }
          // And it must actually work, without leaving /public.
          await autoLink.click();
          await w.page
            .waitForFunction(() => document.body.innerText.includes("A public comment body"), undefined, {
              timeout: 10_000,
            })
            .catch(() => fail(scenario, "the auto-linked identifier did not navigate"));
          if (!w.page.url().includes("#/public/DEMO/issues/DEMO-1")) {
            fail(scenario, `navigation left the public view: ${w.page.url()}`);
          }
        }
        for (const url of w.privateRequests) fail(scenario, `private-API request: ${url}`);
        for (const err of w.pageErrors) fail(scenario, `uncaught page error: ${err}`);
        for (const err of w.consoleErrors) fail(scenario, `console error: ${err}`);
      } catch (e) {
        fail(scenario, String(e));
      } finally {
        await context.close();
      }
      console.log(`${failures.some((f) => f.startsWith(scenario)) ? "FAIL" : "ok  "} ${scenario}`);
    }

    // ---- 4c. pages are the public view's other half ---------------------
    // LIF-471: the sidebar offers Issues and Pages, and both are the real
    // components. A published project's pages are readable the same way.
    {
      const scenario = "public pages";
      const context = await browser.newContext({ viewport: { width: 1280, height: 900 } });
      const w = await watchedPage(context, base);
      try {
        await w.page.goto(`${base}/public/DEMO/pages`, { waitUntil: "load", timeout: 15_000 });
        await w.page
          .waitForFunction(() => document.body.innerText.includes("Public page"), undefined, {
            timeout: 15_000,
          })
          .catch(() => fail(scenario, "the page tree did not list the published page"));

        await w.page.getByText("Public page").first().click();
        await w.page
          .waitForFunction(() => document.body.innerText.includes("Page body text"), undefined, {
            timeout: 10_000,
          })
          .catch(() => fail(scenario, "the page body never rendered"));

        if (!/\/public\/DEMO\/pages\/\d+/.test(w.page.url())) {
          fail(scenario, `opening a page landed on ${w.page.url()}`);
        }
        const body = (await w.page.locator("body").innerText()) ?? "";
        for (const expected of ["Public page", "Page body text", "Read-only"]) {
          if (!body.includes(expected)) {
            fail(scenario, `expected visible text ${JSON.stringify(expected)} not found`);
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

    // ---- 4d. the whole project arrives in one list ----------------------
    // LIF-471 replaced the paged `/issues` endpoint with `/index`, which
    // answers with every live row at once. So the property is no longer "the
    // view walks the pages" but "a project bigger than the old page size
    // renders end to end".
    {
      const scenario = "whole issue list arrives";
      const context = await browser.newContext({ viewport: { width: 1280, height: 900 } });
      const w = await watchedPage(context, base);
      try {
        // Comfortably past the retired 100-row page size.
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

        await w.page.goto(`${base}/public/DEMO/issues`, { waitUntil: "load", timeout: 15_000 });
        // Default grouping is by status, and every seeded row is `backlog`,
        // so they all live in one group.
        for (const title of ["Bulk issue 1", "Bulk issue 120"]) {
          const found = await w.page
            .getByText(title, { exact: true })
            .first()
            .waitFor({ timeout: 15_000 })
            .then(() => true)
            .catch(() => false);
          if (!found) fail(scenario, `${JSON.stringify(title)} never rendered`);
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
          fail(scenario, `only ${rows} issue rows rendered; expected at least 100`);
        }
        const body = (await w.page.locator("body").innerText()) ?? "";
        if (!body.includes("Public issue one")) {
          fail(scenario, "the originally seeded issues are missing from the list");
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

    // The LIF-465 "comment paging" and "load-more navigation race" scenarios
    // are gone with the bespoke public view: comments now render through the
    // ordinary `Comments` thread (newest page plus a "Load older comments"
    // control), which the signed-in smoke already exercises, and the paged
    // `/public/api/.../issues` list endpoint they leaned on no longer exists.

    // ---- 4e. navigating away mid-load does not leak content -------------
    {
      const scenario = "route race";
      const context = await browser.newContext();
      const w = await watchedPage(context, base);
      try {
        await w.page.goto(`${base}/public/DEMO/issues`, { waitUntil: "load", timeout: 15_000 });
        await w.page.waitForLoadState("networkidle", { timeout: 10_000 }).catch(() => {});
        // Bounce between two issues fast enough that responses for an
        // abandoned one land after the next has rendered.
        for (let i = 0; i < 6; i += 1) {
          await w.page.evaluate(() => {
            window.location.hash = "#/public/DEMO/issues/DEMO-3";
          });
          await w.page.evaluate(() => {
            window.location.hash = "#/public/DEMO/issues";
          });
        }
        await w.page.evaluate(() => {
          window.location.hash = "#/public/DEMO/issues/DEMO-1";
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

    // ---- 5. a private project is not reachable --------------------------
    {
      const scenario = "private project stays private";
      const context = await browser.newContext();
      const w = await watchedPage(context, base);
      try {
        await w.page.goto(`${base}/public/PRIV/issues`, { waitUntil: "load", timeout: 15_000 });
        await w.page.waitForLoadState("networkidle", { timeout: 10_000 }).catch(() => {});
        const body = (await w.page.locator("body").innerText()) ?? "";
        if (body.includes("classified-marker-string") || body.includes("Classified issue")) {
          fail(scenario, "an unpublished project's content rendered");
        }
        if (!body.includes("This project isn't public")) {
          fail(scenario, `expected the "isn't public" message, got: ${body.slice(0, 200)}`);
        }
        // Refusing must not have gone looking on the credentialed surface.
        for (const url of w.privateRequests) fail(scenario, `private-API request: ${url}`);

        // A private project and a nonexistent one are the same answer, byte
        // for byte: the surface is not an oracle for what the instance holds.
        const [priv, nope] = await w.page.evaluate(async (b: string) => {
          const read = async (p: string) => {
            const r = await fetch(`${b}/public/api/projects/${p}/index`, { credentials: "omit" });
            return { status: r.status, body: await r.text() };
          };
          return [await read("PRIV"), await read("NOPE")];
        }, base);
        if (priv.status !== 404) {
          fail(scenario, `an unpublished project's index answered ${priv.status}, not 404`);
        }
        if (priv.status !== nope.status || priv.body !== nope.body) {
          fail(
            scenario,
            `a private project is distinguishable from a nonexistent one: ` +
              `${priv.status} ${JSON.stringify(priv.body)} vs ${nope.status} ${JSON.stringify(nope.body)}`,
          );
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
        await w.page.goto(`${base}/public/DEMO/issues`, { waitUntil: "load", timeout: 15_000 });
        await w.page.waitForLoadState("networkidle", { timeout: 10_000 }).catch(() => {});
        if (!(await w.page.locator("body").innerText()).includes("Public issue one")) {
          fail(scenario, "the list did not render before unpublishing");
        }

        const live = new Database(db);
        live.run("UPDATE projects SET is_public = 0 WHERE identifier = 'DEMO'");
        live.close();

        // A direct fetch proves the server closed it; the reload proves the
        // page a reader already had open cannot be refreshed back into life.
        const status = await w.page.evaluate(async (b: string) => {
          const r = await fetch(`${b}/public/api/projects/DEMO/index`, {
            credentials: "omit",
          });
          return r.status;
        }, base);
        if (status !== 404) {
          fail(scenario, `the index endpoint answered ${status} after unpublishing, not 404`);
        }

        await w.page.reload({ waitUntil: "load", timeout: 15_000 });
        await w.page.waitForLoadState("networkidle", { timeout: 10_000 }).catch(() => {});
        const body = (await w.page.locator("body").innerText()) ?? "";
        if (body.includes("Public issue one")) {
          fail(scenario, "the issue still rendered after publication was turned off");
        }
        if (!body.includes("This project isn't public")) {
          fail(scenario, `expected the "isn't public" message, got: ${body.slice(0, 200)}`);
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
      const context = await browser.newContext({ viewport: { width: 1280, height: 900 } });
      const page = await context.newPage();
      try {
        await page.goto(`${base}/DEMO/issues`, { waitUntil: "load", timeout: 15_000 });
        await page.waitForLoadState("networkidle", { timeout: 10_000 }).catch(() => {});
        if (!page.url().includes("/login")) {
          fail(scenario, `an anonymous visitor reached ${page.url()} instead of the login page`);
        }

        // Sign in the way a person does, then confirm the private view still
        // works: the public routes must not have loosened or broken it.
        await page.goto(`${base}/login`, { waitUntil: "load", timeout: 15_000 });
        await page.fill("#login-identity", "public-operator");
        await page.fill("#login-password", PASSWORD);
        await page.click("button[type=submit]");
        await page.waitForURL(`${base}/`, { timeout: 15_000 }).catch(() => {});
        if (page.url().includes("/login")) {
          const text = await page.locator("body").innerText().catch(() => "");
          fail(scenario, `login did not land on the app: ${text.slice(0, 200)}`);
        }

        await page.evaluate(() => {
          window.location.hash = "#/DEMO/issues";
        });
        await page
          .waitForFunction(() => document.body.innerText.includes("Public issue one"), undefined, {
            timeout: 15_000,
          })
          .catch(() => fail(scenario, "the signed-in issue list did not render"));
        if ((await page.getByLabel("More create options").count()) === 0) {
          fail(scenario, "the signed-in list lost its create control");
        }

        // LIF-471: the same account, on the public route. A signed-in reader
        // must see exactly what a stranger sees, which means the credential
        // is not attached and the private API is not consulted at all.
        //
        // A page of its own, in the same (signed-in) context: it carries the
        // session cookie and the token in localStorage, and the public route
        // is the first thing it ever loads, so nothing the private app was
        // doing can be mistaken for something the public view did.
        const publicPage = await context.newPage();
        let watching = true;
        const seen: { url: string; auth: string | undefined }[] = [];
        publicPage.on("request", (req) => {
          if (watching) seen.push({ url: req.url(), auth: req.headers()["authorization"] });
        });
        await publicPage.goto(`${base}/#/public/DEMO/issues`, {
          waitUntil: "load",
          timeout: 15_000,
        });
        await publicPage
          .waitForFunction(() => document.body.innerText.includes("Public issue one"), undefined, {
            timeout: 15_000,
          })
          .catch(() => fail(scenario, "the public route did not render for a signed-in reader"));
        await publicPage.waitForLoadState("networkidle", { timeout: 10_000 }).catch(() => {});
        watching = false;

        if ((await publicPage.evaluate(() => localStorage.getItem("lific_token"))) === null) {
          fail(scenario, "the context under test was not actually signed in");
        }
        for (const req of seen) {
          if (req.url.startsWith(`${base}/public/api`) && req.auth !== undefined) {
            fail(scenario, `a public request carried a credential: ${req.url}`);
          }
          // Same-origin only: index.html loads the app's webfonts from
          // Google on every route, signed-in or not, which is app-wide and
          // predates this feature (see `watchedPage`).
          if (req.url.startsWith(base) && isPrivateRequest(req.url, base)) {
            fail(scenario, `the public view reached the private API: ${req.url}`);
          }
        }

        // And nothing to create with, even though this reader could.
        if ((await publicPage.getByLabel("More create options").count()) > 0) {
          fail(scenario, "the public view offered a create control to a signed-in reader");
        }

        // Leaving again restores the signed-in affordances: the read-only
        // answers the public scope synthesizes must not have poisoned the
        // role cache.
        await publicPage.evaluate(() => {
          window.location.hash = "#/DEMO/issues";
        });
        const restored = await publicPage
          .getByLabel("More create options")
          .first()
          .waitFor({ timeout: 15_000 })
          .then(() => true)
          .catch(() => false);
        if (!restored) {
          fail(scenario, "the create control did not come back after leaving the public view");
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

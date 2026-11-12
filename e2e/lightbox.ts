#!/usr/bin/env bun
// The image lightbox, opened from a real Markdown body: a 4000 px screenshot
// opens fitted, a click zooms it to native size, the mouse drags it, the wheel
// and a two-finger pinch zoom it, Download fetches the original, the arrow
// keys step to the other image, and Escape closes.
import { strict as assert } from "node:assert";
import { resolve } from "node:path";
import { chromium } from "playwright";
import { createServer } from "../web/node_modules/vite/dist/node/index.js";
import { png } from "./png";

const root = resolve(import.meta.dir, "../web");
const fixtureId = root + "/src/LightboxFixture.svelte";
const content = ["![dense log](/api/attachments/1)", "", "text", "", "![second](/api/attachments/2)"].join(
  "\\n",
);
const fixture = `<script>
  import Markdown from './lib/Markdown.svelte';
</script>
<main style="max-width: 700px"><Markdown content={"${content}"} /></main>`;
const originalHtml = await Bun.file(root + "/index.html").text();
const html = originalHtml.replace(
  '<script type="module" src="/src/main.ts"></script>',
  `<script type="module">import {mount} from 'svelte'; import Fixture from '/src/LightboxFixture.svelte'; import '/src/app.css'; mount(Fixture,{target:document.getElementById('app')});</script>`,
);
const server = await createServer({
  root,
  logLevel: "error",
  server: { host: "127.0.0.1", port: 0, strictPort: false },
  plugins: [
    {
      name: "lightbox-fixture",
      enforce: "pre",
      resolveId(id) {
        if (id === "/src/LightboxFixture.svelte") return fixtureId;
      },
      load(id) {
        if (id === fixtureId) return fixture;
      },
      configureServer(vite) {
        vite.middlewares.use(async (req, res, next) => {
          if (req.url?.split("?")[0] !== "/") return next();
          res.setHeader("Content-Type", "text/html");
          res.end(await vite.transformIndexHtml("/", html));
        });
      },
    },
  ],
});
const deadline = setTimeout(() => {
  console.error("Lightbox test deadline exceeded");
  process.exit(1);
}, 90_000);

const original: Record<string, Buffer> = { 1: png(4000, 2000), 2: png(800, 600) };

let browser;
try {
  await server.listen();
  const executablePath = process.env.PLAYWRIGHT_EXECUTABLE_PATH;
  browser = await chromium.launch(
    executablePath ? { headless: true, executablePath } : { headless: true, channel: "chromium" },
  );
  const context = await browser.newContext({
    viewport: { width: 1200, height: 900 },
    acceptDownloads: true,
  });
  const page = await context.newPage();
  page.setDefaultTimeout(8_000);
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  // Playwright tries the most recently added route first.
  await page.route("**/api/**", (route) => route.fulfill({ json: [] }));
  await page.route(/\/api\/.*attachments\/\d+/, (route) => {
    const m = new URL(route.request().url()).pathname.match(/attachments\/(\d+)(\/thumbnail)?$/);
    if (!m || m[2] || !original[m[1]]) return route.fulfill({ status: 404, body: "" });
    return route.fulfill({ contentType: "image/png", body: original[m[1]] });
  });

  await page.goto(server.resolvedUrls!.local[0]);
  const bodyImage = page.locator(".prose img[alt='dense log']");
  await bodyImage.waitFor();
  await bodyImage.click();

  const box = page.locator(".lightbox__img");
  await page.locator(".lightbox__img.lightbox__img--ready").waitFor();
  const rect = () => box.evaluate((el) => el.getBoundingClientRect().toJSON());
  const zoom = () => page.locator(".lightbox__zoom").textContent();

  // Fitted: the 1200x900 window less 32 px each side is 1136 wide, which binds.
  let r = await rect();
  assert.ok(Math.abs(r.width - 1136) <= 1, `fit width ${r.width}`);
  assert.equal(await zoom(), "28%");

  // Click: native size, about the click point.
  await page.mouse.click(600, 450);
  r = await rect();
  assert.ok(Math.abs(r.width - 4000) <= 1, `native width ${r.width}`);
  assert.equal(await zoom(), "100%");

  // Drag pans without closing or zooming back.
  const before = await rect();
  await page.mouse.move(600, 450);
  await page.mouse.down();
  await page.mouse.move(450, 350, { steps: 5 });
  await page.mouse.move(300, 250, { steps: 5 });
  await page.mouse.up();
  r = await rect();
  assert.ok(Math.abs(r.x - before.x + 300) <= 1 && Math.abs(r.y - before.y + 200) <= 1, `pan ${JSON.stringify([before, r])}`);
  assert.equal(await zoom(), "100%", "a drag is not a click");
  assert.equal(await page.locator(".lightbox").count(), 1, "a drag does not close");

  // Wheel zooms in past native size.
  await page.mouse.move(600, 450);
  await page.mouse.wheel(0, -400);
  await page.waitForFunction(() => Number(document.querySelector(".lightbox__zoom")?.textContent?.replace("%", "")) > 100);

  // Back to fit, then a two-finger pinch outwards zooms in.
  await page.keyboard.press("0");
  assert.equal(await zoom(), "28%");
  const cdp = await context.newCDPSession(page);
  const touch = (type: string, spread: number) =>
    cdp.send("Input.dispatchTouchEvent", {
      type,
      touchPoints:
        type === "touchEnd"
          ? []
          : [
              { x: 600 - spread, y: 450, id: 1 },
              { x: 600 + spread, y: 450, id: 2 },
            ],
    });
  await touch("touchStart", 50);
  for (const spread of [80, 120, 160, 200]) await touch("touchMove", spread);
  await touch("touchEnd", 0);
  const pinched = Number((await zoom())!.replace("%", ""));
  assert.ok(pinched >= 100, `pinch zoom ${pinched}%`);

  // Download the original.
  const [download] = await Promise.all([
    page.waitForEvent("download"),
    page.getByRole("button", { name: "Download" }).or(page.getByRole("link", { name: "Download" })).click(),
  ]);
  assert.match(download.url(), /\/api\/attachments\/1$/);

  // Arrow to the next image on the page, then close.
  await page.keyboard.press("ArrowRight");
  await page.locator(".lightbox__img[alt='second'].lightbox__img--ready").waitFor();
  assert.equal(await page.locator(".lightbox__count").textContent(), "2 / 2");
  r = await rect();
  assert.ok(Math.abs(r.width - 800) <= 1, `second image at its own size ${r.width}`);
  await page.keyboard.press("Escape");
  await page.locator(".lightbox").waitFor({ state: "detached" });
  assert.deepEqual(errors, []);
  console.log("lightbox: ok");
} finally {
  clearTimeout(deadline);
  await browser?.close();
  await server.close();
}
process.exit(0);

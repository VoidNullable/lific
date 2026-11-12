#!/usr/bin/env bun
// `![alt|N](url)` image widths, rendered by the real Markdown component in
// signed-in and public scope: the width is a ceiling, a small image is not
// stretched, a width past the thumbnail edge loads the original, and a lone
// titled image becomes a figure whose caption wraps at the image edge.
import { strict as assert } from "node:assert";
import { resolve } from "node:path";
import { deflateSync } from "node:zlib";
import { chromium } from "playwright";
import { createServer } from "../web/node_modules/vite/dist/node/index.js";

const CRC_TABLE = Array.from({ length: 256 }, (_, n) => {
  let c = n;
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  return c >>> 0;
});
function crc32(bytes: Uint8Array): number {
  let c = 0xffffffff;
  for (const b of bytes) c = CRC_TABLE[(c ^ b) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}
function chunk(type: string, data: Uint8Array): Buffer {
  const body = Buffer.concat([Buffer.from(type, "ascii"), Buffer.from(data)]);
  const out = Buffer.alloc(body.length + 8);
  out.writeUInt32BE(data.length, 0);
  body.copy(out, 4);
  out.writeUInt32BE(crc32(body), body.length + 4);
  return out;
}
function png(width: number, height: number): Buffer {
  const header = Buffer.alloc(13);
  header.writeUInt32BE(width, 0);
  header.writeUInt32BE(height, 4);
  header[8] = 8; // bit depth
  header[9] = 2; // truecolor
  const row = Buffer.alloc(1 + width * 3, 0x80);
  row[0] = 0;
  const raw = Buffer.concat(Array.from({ length: height }, () => row));
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", header),
    chunk("IDAT", deflateSync(raw)),
    chunk("IEND", new Uint8Array()),
  ]);
}

const root = resolve(import.meta.dir, "../web");
const fixtureId = root + "/src/ImageWidthsFixture.svelte";
const content = [
  "![wide shot|240](/api/attachments/1)",
  "",
  "![icon|240](/api/attachments/2)",
  "",
  "![wide shot|960](/api/attachments/1)",
  "",
  "![wide shot](/api/attachments/1)",
  "",
  '![captioned|240](/api/attachments/1 \\"The login dialog after a failed attempt, which wraps\\")',
].join("\\n");
const fixture = `<script>
  import Markdown from './lib/Markdown.svelte';
  import { setPublicProject } from './lib/publicScope';
  if (location.search.includes('public')) setPublicProject('TST');
</script>
<main style="width: 700px"><Markdown content={"${content}"} /></main>`;
const originalHtml = await Bun.file(root + "/index.html").text();
const html = originalHtml.replace(
  '<script type="module" src="/src/main.ts"></script>',
  `<script type="module">import {mount} from 'svelte'; import Fixture from '/src/ImageWidthsFixture.svelte'; import '/src/app.css'; mount(Fixture,{target:document.getElementById('app')});</script>`,
);
const server = await createServer({
  root,
  logLevel: "error",
  server: { host: "127.0.0.1", port: 0, strictPort: false },
  plugins: [
    {
      name: "image-widths-fixture",
      enforce: "pre",
      resolveId(id) {
        if (id === "/src/ImageWidthsFixture.svelte") return fixtureId;
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
  console.error("Image width test deadline exceeded");
  process.exit(1);
}, 90_000);

// Attachment 1 is 1600x900 with a 480x270 thumbnail; attachment 2 is a
// 100x80 icon with no thumbnail, as the server does below 480 px.
const original = { 1: png(1600, 900), 2: png(100, 80) } as Record<string, Buffer>;
const thumbnail = { 1: png(480, 270) } as Record<string, Buffer>;

let browser;
try {
  await server.listen();
  const executablePath = process.env.PLAYWRIGHT_EXECUTABLE_PATH;
  browser = await chromium.launch(
    executablePath ? { headless: true, executablePath } : { headless: true, channel: "chromium" },
  );
  const page = await browser.newPage({ viewport: { width: 1200, height: 2400 } });
  page.setDefaultTimeout(8_000);
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  // Playwright tries the most recently added route first.
  await page.route("**/api/**", (route) => route.fulfill({ json: [] }));
  await page.route(/\/api\/.*attachments\/\d+/, (route) => {
    const m = new URL(route.request().url()).pathname.match(/attachments\/(\d+)(\/thumbnail)?$/);
    const body = m && (m[2] ? thumbnail[m[1]] : original[m[1]]);
    if (!body) return route.fulfill({ status: 404, body: "" });
    return route.fulfill({ contentType: "image/png", body });
  });

  for (const scope of ["signed-in", "public"]) {
    await page.goto(server.resolvedUrls!.local[0] + (scope === "public" ? "?public" : ""));
    await page.locator(".prose img").nth(4).waitFor();
    await page.waitForFunction(() =>
      Array.from(document.querySelectorAll<HTMLImageElement>(".prose img")).every(
        (img) => img.complete && img.naturalWidth > 0,
      ),
    );
    const images = await page.$$eval(".prose img", (imgs) =>
      imgs.map((img) => ({
        alt: img.getAttribute("alt"),
        width: Math.round(img.getBoundingClientRect().width),
        thumbnail: (img as HTMLImageElement).currentSrc.includes("/thumbnail"),
      })),
    );
    const report = `${scope}: ${JSON.stringify(images)}`;
    // Rendered widths include the 1 px border on each side.
    const expected = [
      { alt: "wide shot", width: 240, thumbnail: true },
      { alt: "icon", width: 100, thumbnail: false },
      { alt: "wide shot", width: 700, thumbnail: false },
      { alt: "wide shot", width: 480, thumbnail: true },
      { alt: "captioned", width: 240, thumbnail: true },
    ];
    assert.equal(images.length, expected.length, report);
    expected.forEach((want, i) => {
      const got = images[i];
      assert.equal(got.alt, want.alt, report);
      assert.equal(got.thumbnail, want.thumbnail, report);
      assert.ok(Math.abs(got.width - want.width) <= 2, report);
    });
    // The caption wraps inside the figure, which is no wider than its image.
    const figure = await page.$eval(".prose figure.md-figure", (fig) => ({
      width: Math.round(fig.getBoundingClientRect().width),
      caption: fig.querySelector("figcaption")?.textContent,
      captionLines: Math.round(
        fig.querySelector("figcaption")!.getBoundingClientRect().height /
          parseFloat(getComputedStyle(fig.querySelector("figcaption")!).lineHeight),
      ),
    }));
    assert.ok(Math.abs(figure.width - 240) <= 2, JSON.stringify(figure));
    assert.equal(figure.caption, "The login dialog after a failed attempt, which wraps");
    assert.ok(figure.captionLines >= 2, JSON.stringify(figure));
    console.log(report, JSON.stringify(figure));
  }
  assert.deepEqual(errors, []);
  console.log("image widths: ok");
} finally {
  clearTimeout(deadline);
  await browser?.close();
  await server.close();
}
process.exit(0);

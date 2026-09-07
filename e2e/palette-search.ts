/**
 * Command palette local-first search (LIF-445).
 *
 * The unit tests cover the scoring helper; they cannot see a duplicate
 * `{#each}` key, a stale response repainting a closed palette, or whether
 * anything actually rendered before the network answered. Those are the
 * three things that broke in review, so they are checked here, in a real
 * browser, against a real server.
 *
 * Seeds its own project (PAL) through the authenticated API from inside the
 * page, so it does not disturb the DEMO fixtures the rest of the smoke test
 * asserts on.
 */
import { strict as assert } from "node:assert";
import type { BrowserContext, Page } from "playwright";

/** Issues whose titles all contain "warmup" — enough to clear the
 *  LOCAL_HIT_SERVER_THRESHOLD of 5 and prove the server is never asked. */
const WARMUP_ISSUES = 7;

/** A word that exists ONLY in an issue description, past the 200-character
 *  preview cap, so the read model cannot possibly know about it. Finding it
 *  proves the server FTS still runs when the local answer is empty. */
const BODY_ONLY_TOKEN = "zarquontoken";

/** A second token, shared by two local rows (one issue, one page) and one
 *  issue that only the server can find. Two local hits is under the
 *  threshold, so the FTS runs and all three land in one merged list — which
 *  is the only way to see whether a server issue can slip above a local
 *  page. */
const MIXED_TOKEN = "mixedtoken";

const PLACEHOLDER = /^Jump or act/;

type Seeded = { projectId: number };

async function seed(page: Page, base: string): Promise<Seeded> {
  return page.evaluate(
    async ({ warmupCount, token, mixed }) => {
      const auth = localStorage.getItem("lific_token");
      if (!auth) throw new Error("no lific_token in localStorage — not signed in");
      const post = async (path: string, body: unknown) => {
        const res = await fetch(`/api${path}`, {
          method: "POST",
          headers: { "Content-Type": "application/json", Authorization: `Bearer ${auth}` },
          body: JSON.stringify(body),
        });
        const json = await res.json();
        if (!res.ok) throw new Error(`POST ${path} -> ${res.status} ${JSON.stringify(json)}`);
        return json as { id: number };
      };

      const project = await post("/projects", { name: "Palette", identifier: "PAL" });
      for (let i = 1; i <= warmupCount; i++) {
        await post("/issues", {
          project_id: project.id,
          title: `Palette warmup ${i}`,
          description: `Seeded for the palette search check, number ${i}.`,
          status: "active",
        });
      }
      // The preview is the first non-empty line capped at 200 chars, so a
      // 360-character opening line hides everything after it from the read
      // model. The token lives two lines down.
      await post("/issues", {
        project_id: project.id,
        title: "Cold storage note",
        description: `${"lorem ipsum dolor sit amet ".repeat(14)}\n\n${token} appears only in the body.`,
        status: "backlog",
      });
      await post("/pages", {
        project_id: project.id,
        title: "Palette warmup handbook",
        content: "# Palette warmup handbook\n\nHow the warm read model is searched.",
      });

      // The mixed-source trio: two rows the read model can see (an issue and
      // a page, both with the token in the title) and one only the FTS can
      // reach (token buried past the preview cap).
      await post("/issues", {
        project_id: project.id,
        title: `${mixed} local issue`,
        description: "Found in memory, by title.",
        status: "active",
      });
      await post("/pages", {
        project_id: project.id,
        title: `${mixed} local page`,
        content: `# ${mixed} local page\n\nFound in memory, by title.`,
      });
      await post("/issues", {
        project_id: project.id,
        title: "Buried reference note",
        description: `${"lorem ipsum dolor sit amet ".repeat(14)}\n\n${mixed} lives only in this body.`,
        status: "active",
      });
      return { projectId: project.id };
    },
    { warmupCount: WARMUP_ISSUES, token: BODY_ONLY_TOKEN, mixed: MIXED_TOKEN },
  );
}

/** Rows the palette is currently showing, as trimmed text. */
function rows(page: Page) {
  return page.evaluate(() =>
    [...document.querySelectorAll("[data-flat-idx]")].map((el) =>
      (el as HTMLElement).innerText.replace(/\s+/g, " ").trim(),
    ),
  );
}

async function openPalette(page: Page) {
  await page.keyboard.press("Control+k");
  const input = page.getByPlaceholder(PLACEHOLDER);
  await input.waitFor({ state: "visible", timeout: 5_000 });
  return input;
}

export async function checkPaletteSearch(context: BrowserContext, base: string) {
  const page = await context.newPage();
  const consoleErrors: string[] = [];
  const pageErrors: string[] = [];
  page.on("console", (m) => {
    if (m.type() === "error") consoleErrors.push(m.text());
  });
  page.on("pageerror", (e) => pageErrors.push(String(e)));

  let searchRequests = 0;
  page.on("request", (r) => {
    if (new URL(r.url()).pathname === "/api/search") searchRequests++;
  });

  try {
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.goto(`${base}/`, { waitUntil: "load", timeout: 15_000 });
    await seed(page, base);

    // Warm the read model: the list route bootstraps it, and the rows
    // rendering is the observable proof that status === "ready".
    await page.goto(`${base}/PAL/issues`, { waitUntil: "load", timeout: 15_000 });
    await page.waitForFunction(
      (n) => document.querySelectorAll("[data-issue-index]").length >= n,
      WARMUP_ISSUES,
      { timeout: 15_000 },
    );
    await page.waitForLoadState("networkidle", { timeout: 10_000 }).catch(() => {});

    // ── 1. Local hits render with no network round trip ───────────────
    {
      const input = await openPalette(page);
      const before = searchRequests;
      const t0 = Date.now();
      await input.fill("warmup");
      await page.waitForFunction(
        () =>
          [...document.querySelectorAll("[data-flat-idx]")].filter((el) =>
            (el as HTMLElement).innerText.includes("Palette warmup"),
          ).length >= 6,
        undefined,
        { timeout: 2_000 },
      );
      const renderedMs = Date.now() - t0;
      assert.equal(
        searchRequests - before,
        0,
        `local results must render before any /api/search (saw ${searchRequests - before})`,
      );

      // Past the 120ms debounce plus generous slack: the FTS must never fire.
      await page.waitForTimeout(700);
      assert.equal(
        searchRequests - before,
        0,
        `FTS must be skipped when local hits >= 5 (saw ${searchRequests - before})`,
      );

      const visible = await rows(page);
      const pageHits = visible.filter((r) => r.includes("Palette warmup handbook")).length;
      const issueHits = visible.filter(
        (r) => /Palette warmup \d/.test(r) && r.includes("PAL-"),
      ).length;
      assert(issueHits >= 6, `expected >= 6 local issue hits, got ${issueHits}`);
      assert.equal(pageHits, 1, "the page must survive a flood of matching issues");
      console.log(
        `ok   palette local search: ${issueHits} issues + ${pageHits} page in ${renderedMs}ms, 0 /api/search`,
      );
      await page.keyboard.press("Escape");
    }

    // ── 2. Exact reference: one row, not two, and it navigates ────────
    {
      const input = await openPalette(page);
      await input.fill("PAL-3");
      // The identifier fast path, the read model and the FTS all answer with
      // PAL-3; before the dedupe they emitted duplicate keyed rows.
      await page.waitForTimeout(900);
      const visible = await rows(page);
      const refRows = visible.filter((r) => /\bPAL-3\b/.test(r));
      assert.equal(
        refRows.length,
        1,
        `PAL-3 must appear exactly once, got ${refRows.length}: ${JSON.stringify(refRows)}`,
      );
      assert.equal(
        new Set(visible).size,
        visible.length,
        `duplicate rows in the palette: ${JSON.stringify(visible)}`,
      );
      await page.keyboard.press("Enter");
      await page.waitForURL(/#\/PAL\/issues\/PAL-3$/, { timeout: 10_000 });
      console.log("ok   palette exact reference: single row, navigates to PAL-3");
    }

    // ── 3. A body-only token still reaches the server ─────────────────
    await page.goto(`${base}/PAL/issues`, { waitUntil: "load", timeout: 15_000 });
    await page.waitForFunction(
      (n) => document.querySelectorAll("[data-issue-index]").length >= n,
      WARMUP_ISSUES,
      { timeout: 15_000 },
    );
    {
      const input = await openPalette(page);
      const before = searchRequests;
      await input.fill(BODY_ONLY_TOKEN);
      // Assert on the palette's own rows, not the body: the issue list is
      // still rendered behind the overlay and already names this issue.
      await page.waitForFunction(
        () =>
          [...document.querySelectorAll("[data-flat-idx]")].some((el) =>
            (el as HTMLElement).innerText.includes("Cold storage note"),
          ),
        undefined,
        { timeout: 10_000 },
      );
      const fired = searchRequests - before;
      assert(fired >= 1, "an unmatchable-locally query must fall through to the server");
      console.log(`ok   palette server fallback: body-only token found via ${fired} /api/search`);
      await page.keyboard.press("Escape");
    }

    // ── 3b. Every server row sits below every local row ───────────────
    // Grouping by kind used to be able to interleave the two sources: a
    // server ISSUE outranked a local PAGE simply because the Issues group
    // scored higher. Local issue, local page and server issue in one merged
    // list is the arrangement that exposes it.
    {
      const input = await openPalette(page);
      await input.fill(MIXED_TOKEN);
      await page.waitForFunction(
        () =>
          [...document.querySelectorAll("[data-flat-idx]")].some((el) =>
            (el as HTMLElement).innerText.includes("Buried reference note"),
          ),
        undefined,
        { timeout: 10_000 },
      );

      // Walk the results list in document order, keeping the group headers
      // so the "(server)" boundary itself can be located.
      const sequence = await page.evaluate(() => {
        const first = document.querySelector("[data-flat-idx]");
        if (!first?.parentElement) return [] as { row: boolean; text: string }[];
        return [...first.parentElement.children].map((el) => ({
          row: el.hasAttribute("data-flat-idx"),
          text: (el as HTMLElement).innerText.replace(/\s+/g, " ").trim(),
        }));
      });
      const at = (needle: string) => sequence.findIndex((s) => s.text.includes(needle));

      const localIssue = at("local issue");
      const localPage = at("local page");
      const serverIssue = at("Buried reference note");
      // The group label is uppercased by CSS, and `innerText` reflects that,
      // so match it case-insensitively.
      const serverHeader = sequence.findIndex((s) => !s.row && /\(server\)/i.test(s.text));
      const dump = JSON.stringify(sequence.map((s) => `${s.row ? "" : "# "}${s.text.slice(0, 48)}`));

      assert(localIssue >= 0, `local issue row missing: ${dump}`);
      assert(localPage >= 0, `local page row missing: ${dump}`);
      assert(serverIssue >= 0, `server-only row missing: ${dump}`);
      assert(
        serverIssue > localIssue,
        `server issue rendered above the local issue: ${dump}`,
      );
      assert(
        serverIssue > localPage,
        `server issue rendered above the local page: ${dump}`,
      );
      assert(serverHeader >= 0, `no "(server)" group header: ${dump}`);
      assert(
        serverHeader > localIssue && serverHeader > localPage,
        `the "(server)" section opened before the local rows ended: ${dump}`,
      );
      // Nothing local may appear after the server boundary.
      const strays = sequence
        .slice(serverHeader)
        .filter((s) => s.row && /local (issue|page)/.test(s.text));
      assert.equal(strays.length, 0, `local rows leaked into the server section: ${dump}`);

      console.log(
        `ok   palette source order: local issue+page above server issue (server header at ${serverHeader})`,
      );
      await page.keyboard.press("Escape");
    }

    // ── 4. Stale responses never land ─────────────────────────────────
    // Hold /api/search open, then move the query, the project, and the
    // palette itself out from under the in-flight request.
    await page.route("**/api/search*", async (route) => {
      await new Promise((r) => setTimeout(r, 1_500));
      await route.continue();
    });
    try {
      // 4a. Query changes while the response is in flight.
      {
        const input = await openPalette(page);
        await input.fill(BODY_ONLY_TOKEN);
        await page.waitForTimeout(300); // debounce fired, request in flight
        await input.fill("warmup");
        await page.waitForTimeout(2_200); // the held response lands here
        const visible = await rows(page);
        assert(
          !visible.some((r) => r.includes("Cold storage note")),
          `stale response repainted the palette: ${JSON.stringify(visible)}`,
        );
        assert(
          visible.some((r) => r.includes("Palette warmup ")),
          "the current query's local results were dropped",
        );
        await page.keyboard.press("Escape");
        console.log("ok   palette stale guard: query change discards the in-flight response");
      }

      // 4b. Project changes under an open palette. The palette's server
      // search is deliberately cross-project, so PAL-8 legitimately stays
      // visible from DEMO — what must not happen is the discarded PAL
      // response merging with the re-issued DEMO one and rendering the row
      // twice, or the local rows of one project mixing into the other's.
      {
        const input = await openPalette(page);
        await input.fill(BODY_ONLY_TOKEN);
        await page.waitForTimeout(300); // request in flight against PAL
        const before = searchRequests;
        await page.evaluate(() => {
          location.hash = "#/DEMO/issues";
        });
        await page.waitForTimeout(2_500); // both responses land in here
        assert(
          searchRequests > before,
          "the project change must re-issue the search, not reuse the PAL request",
        );
        const visible = await rows(page);
        const hits = visible.filter((r) => r.includes("Cold storage note"));
        assert.equal(
          hits.length,
          1,
          `the discarded PAL response merged with the DEMO one: ${JSON.stringify(visible)}`,
        );
        assert.equal(
          new Set(visible).size,
          visible.length,
          `duplicate rows after a project change: ${JSON.stringify(visible)}`,
        );
        await page.keyboard.press("Escape");
        console.log("ok   palette stale guard: project change re-runs without double-rendering");
      }

      // 4c. Close and reopen while the response is in flight.
      {
        await page.goto(`${base}/PAL/issues`, { waitUntil: "load", timeout: 15_000 });
        await page.waitForFunction(
          (n) => document.querySelectorAll("[data-issue-index]").length >= n,
          WARMUP_ISSUES,
          { timeout: 15_000 },
        );
        const input = await openPalette(page);
        await input.fill(BODY_ONLY_TOKEN);
        await page.waitForTimeout(300);
        await page.keyboard.press("Escape");
        await page.getByPlaceholder(PLACEHOLDER).waitFor({ state: "detached", timeout: 5_000 });
        await page.waitForTimeout(1_800); // the held response lands while closed
        await openPalette(page);
        await page.waitForTimeout(300);
        const visible = await rows(page);
        assert(
          !visible.some((r) => r.includes("Cold storage note")),
          `a response from the previous session repainted the reopened palette: ${JSON.stringify(visible)}`,
        );
        await page.keyboard.press("Escape");
        console.log("ok   palette stale guard: close/reopen discards the in-flight response");
      }
    } finally {
      await page.unroute("**/api/search*");
    }

    // ── 5. Phone layout ───────────────────────────────────────────────
    // The palette goes full-screen below 640px and swaps `esc` for a real
    // close button, so the local pass is worth re-running there.
    {
      await page.setViewportSize({ width: 390, height: 844 });
      await page.goto(`${base}/PAL/issues`, { waitUntil: "load", timeout: 15_000 });
      await page.waitForFunction(
        (n) => document.querySelectorAll("[data-issue-index]").length >= n,
        WARMUP_ISSUES,
        { timeout: 15_000 },
      );
      const input = await openPalette(page);
      const before = searchRequests;
      await input.fill("warmup");
      await page.waitForFunction(
        () =>
          [...document.querySelectorAll("[data-flat-idx]")].filter((el) =>
            (el as HTMLElement).innerText.includes("Palette warmup"),
          ).length >= 6,
        undefined,
        { timeout: 2_000 },
      );
      await page.waitForTimeout(700);
      assert.equal(searchRequests - before, 0, "FTS must be skipped on mobile too");
      assert.equal(
        await page.evaluate(() => document.documentElement.scrollWidth > innerWidth),
        false,
        "palette overflows horizontally at 390px",
      );
      await page.getByRole("button", { name: "Close search" }).click();
      await page.getByPlaceholder(PLACEHOLDER).waitFor({ state: "detached", timeout: 5_000 });
      console.log("ok   palette local search on 390px, close button dismisses");
      await page.setViewportSize({ width: 1440, height: 900 });
    }

    assert.equal(pageErrors.length, 0, `uncaught page errors:\n${pageErrors.join("\n")}`);
    assert.equal(consoleErrors.length, 0, `console errors:\n${consoleErrors.join("\n")}`);
  } finally {
    await page.close();
  }
}

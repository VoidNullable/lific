import { strict as assert } from "node:assert";
import { join } from "node:path";
import type { BrowserContext } from "playwright";

export async function checkDiscordFeedback(context: BrowserContext, base: string) {
  for (const width of [1440, 390]) {
    const page = await context.newPage();
    const errors: string[] = [];
    page.on("pageerror", (error) => errors.push(String(error)));
    try {
      await page.setViewportSize({ width, height: 900 });
      await page.goto(`${base}/DEMO/issues`);
      const rows = page.locator("[data-issue-index]");
      await rows.nth(2).waitFor();
      const selected = page.locator('[data-issue-index] [role="checkbox"][aria-checked="true"]');
      const all = page.getByRole("checkbox", { name: /Select all visible issues/ });
      const checkbox = page.getByRole("checkbox", { name: "Select DEMO-1", exact: true });
      assert.equal(await checkbox.evaluate((el) => getComputedStyle(el).opacity), "1");
      const initialUrl = page.url();
      await rows.first().click({ position: { x: 2, y: 2 } });
      assert.equal(page.url(), initialUrl, "row whitespace must not navigate");
      await page.keyboard.press("Control+a");
      await page.waitForFunction(() => document.querySelectorAll('[data-issue-index] [aria-checked="true"]').length === 3);
      assert.equal(await all.isChecked(), true);
      await all.uncheck();
      assert.equal(await selected.count(), 0);
      await checkbox.click();
      await page.getByRole("checkbox", { name: "Select DEMO-2", exact: true }).click();
      assert.equal(await selected.count(), 2);
      assert.equal(await all.evaluate((el: HTMLInputElement) => el.indeterminate), true);

      const downloadEvent = page.waitForEvent("download");
      await page.getByRole("button", { name: "Export", exact: true }).click();
      const download = await downloadEvent;
      assert.equal(download.suggestedFilename(), "DEMO-selected-issues.md");
      const text = await Bun.file((await download.path())!).text();
      assert(text.includes("Smoke issue") && text.includes("Second smoke issue") && text.includes("First smoke comment"));
      assert(!text.includes("Excluded smoke issue"));
      await page.getByRole("button", { name: "Export", exact: true }).waitFor({ state: "visible" });
      await page.route("**/api/export/issues/*", (route) => route.fulfill({ status: 503, body: "unavailable" }));
      let partialDownload = false;
      page.on("download", () => { partialDownload = true; });
      await page.getByRole("button", { name: "Export", exact: true }).click();
      await page.getByRole("alert").filter({ hasText: "Could not export" }).waitFor();
      assert.equal(partialDownload, false);
      await page.unroute("**/api/export/issues/*");
      await page.getByTitle("Clear selection  ·  Esc").click();

      // Collapsed groups must not join the select-all set.
      const backlog = page.getByRole("button", { name: /^Backlog\s+1$/i });
      await backlog.click();
      await all.check();
      assert.equal(await selected.count(), 2);
      await all.uncheck();
      await backlog.click();

      await rows.first().click({ position: { x: 2, y: 2 } });
      await page.keyboard.press("/");
      const search = page.getByPlaceholder("Search issues...");
      await search.fill("Second smoke");
      await page.waitForFunction(() => document.querySelectorAll("[data-issue-index]").length === 1);
      await search.press("Control+a");
      assert.equal(await search.evaluate((el: HTMLInputElement) => el.selectionEnd! - el.selectionStart!), "Second smoke".length);
      assert.equal(await selected.count(), 0, "Ctrl+A in search must select text, not rows");
      await all.check();
      assert.equal(await selected.count(), 1);
      await all.uncheck();
      await search.fill("");
      await search.press("Escape");
      await rows.nth(2).waitFor();

      const overflow = await page.evaluate(() => document.documentElement.scrollWidth > innerWidth);
      assert.equal(overflow, false, `list overflow at ${width}px`);
      assert(!(await page.locator("body").innerText()).includes("Lucide:Terminal"), "malformed icon leaked text");
      if (process.env.LIFIC_SCREENSHOT_DIR) await page.screenshot({ path: join(process.env.LIFIC_SCREENSHOT_DIR, `lific-discord-list-${width}.png`) });
      await page.getByRole("button", { name: "Smoke issue", exact: true }).click();
      await page.waitForURL(/DEMO-1$/);
      assert.equal(errors.length, 0, errors.join("\n"));
      console.log(`ok   Discord issue selection, export, icons (${width}px)`);
    } finally {
      await page.close();
    }
  }

  const page = await context.newPage();
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(String(error)));
  try {
    await page.setViewportSize({ width: 390, height: 900 });
    await page.goto(`${base}/settings`);
    for (const id of ["codex-laptop", "codex-desktop", "custom-agent"]) {
      await page.getByRole("button", { name: "Add custom or named connection" }).click();
      await page.getByLabel("Config template").selectOption(id.startsWith("codex") ? "codex" : "");
      await page.getByLabel("Connection ID", { exact: true }).fill(id);
      await page.getByLabel("Display name (optional)").fill(id);
      await page.getByRole("button", { name: "Connect agent", exact: true }).click();
      const dialog = page.getByRole("dialog");
      await dialog.getByRole("button", { name: "Copy key", exact: true }).waitFor();
      if (id === "custom-agent") {
        assert((await dialog.innerText()).includes("HTTP MCP settings"));
        assert(!(await dialog.innerText()).includes("same everywhere"));
      }
      await dialog.getByRole("button", { name: "Done", exact: true }).click();
    }
    await page.reload();
    const laptop = page.locator('[data-connection-id="codex-laptop"]');
    const desktop = page.locator('[data-connection-id="codex-desktop"]');
    await laptop.getByRole("button", { name: "Disconnect", exact: true }).click();
    await laptop.getByRole("button", { name: "Reconnect", exact: true }).waitFor();
    assert(await desktop.getByRole("button", { name: "Disconnect", exact: true }).isVisible());
    await laptop.getByRole("button", { name: "Reconnect", exact: true }).click();
    await page.getByRole("dialog").getByRole("button", { name: "Copy key", exact: true }).waitFor();
    await page.getByRole("dialog").getByRole("button", { name: "Done", exact: true }).click();
    assert.equal(await laptop.count(), 1);
    await laptop.getByRole("button", { name: "Disconnect", exact: true }).click();
    await laptop.getByRole("button", { name: "Remove", exact: true }).click();
    await laptop.waitFor({ state: "detached" });
    assert(await desktop.getByRole("button", { name: "Disconnect", exact: true }).isVisible());
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false);
    if (process.env.LIFIC_SCREENSHOT_DIR) await page.screenshot({ path: join(process.env.LIFIC_SCREENSHOT_DIR, "lific-discord-tools-390.png"), fullPage: true });
    assert.equal(errors.length, 0, errors.join("\n"));
    console.log("ok   Discord custom/named connection lifecycle (390px)");
  } finally {
    await page.close();
  }
}

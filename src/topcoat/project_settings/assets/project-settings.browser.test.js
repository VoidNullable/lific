const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
test(
  "headless project administration edits the shared identity and rolls back failed group assignment",
  { skip: !process.env.PLAYWRIGHT_EXECUTABLE_PATH },
  async () => {
    const { chromium } = await import(
      path.resolve(
        __dirname,
        "../../../..",
        "e2e/node_modules/playwright/index.mjs",
      )
    );
    const browser = await chromium.launch({
      headless: true,
      executablePath: process.env.PLAYWRIGHT_EXECUTABLE_PATH,
    });
    try {
      const page = await browser.newPage();
      page.setDefaultTimeout(5000);
      const errors = [];
      page.on("pageerror", (error) => errors.push(error.message));
      const html =
        '<html><head><title>Project settings</title></head><body><header><div data-project-identity><h1>Old name</h1></div></header><section data-topcoat-project-settings="settings" data-project-identifier="LIF"><p data-project-settings-status role="status"></p><p data-project-settings-error role="alert"></p><p data-project-settings-warning role="status"></p><div data-project-settings-content></div></section></body></html>';
      await page.route("http://lific.test/**", (route) =>
        route.fulfill({ contentType: "text/html", body: html }),
      );
      await page.goto("http://lific.test/LIF/overview");
      await page.addScriptTag({
        content: fs.readFileSync(`${__dirname}/project-settings.js`, "utf8"),
      });
      await page.evaluate(() => {
        window.calls = [];
        window.role = "lead";
        window.project = {
          id: 4,
          identifier: "LIF",
          name: "Lific",
          description: "An issue tracker",
          emoji: null,
          lead_user_id: 1,
          is_public: false,
        };
        const session = {
          state: { publicProject: null, user: { id: 1 } },
          request: async (path, options = {}) => {
            window.calls.push({ path, ...options });
            if (path === "/projects/4" && options.method === "PUT") {
              window.project = {
                ...window.project,
                ...JSON.parse(options.body),
              };
              return { ok: true, data: window.project };
            }
            if (path === "/project-groups/assign")
              return { ok: false, error: "Group was removed", status: 404 };
            return {
              ok: true,
              data:
                path === "/projects"
                  ? [window.project]
                  : path === "/project-groups"
                    ? [
                        {
                          id: 1,
                          name: "Work",
                          sort_order: 0,
                          project_ids: [4],
                        },
                        {
                          id: 2,
                          name: "Personal",
                          sort_order: 1,
                          project_ids: [],
                        },
                      ]
                    : path === "/projects/4/my-role"
                      ? { role: window.role, enforced: true, is_admin: false }
                      : path === "/project-archives"
                        ? { can_import: true, max_upload_bytes: 100 }
                        : path === "/users"
                          ? [{ id: 1, username: "me" }]
                          : [],
            };
          },
        };
        window.session = session;
        window.app = LificTopcoatProjectSettings.attach(
          document.querySelector("[data-topcoat-project-settings]"),
          { session },
        );
      });
      await page.getByRole("button", { name: "Lific", exact: true }).click();
      await page
        .locator("[data-project-identity]")
        .getByLabel("Name")
        .fill("New project");
      await page
        .locator("[data-project-identity]")
        .getByRole("button", { name: "Save", exact: true })
        .click();
      await page.waitForFunction(
        () =>
          document.querySelector("[data-project-identity] h1")?.textContent ===
          "New project",
      );
      assert.equal(await page.locator("h1").count(), 1);
      assert.deepEqual(errors, []);
      assert.ok(
        await page.locator("form[data-form=assignment]").count(),
        await page.locator("body").innerHTML(),
      );
      const assignment = page.locator("form[data-form=assignment]");
      await assignment.getByLabel("Group", { exact: true }).selectOption("2");
      await assignment.getByRole("button", { name: "Save group" }).click();
      await page.waitForFunction(
        () =>
          document.querySelector("[data-project-settings-error]")
            .textContent === "Group was removed",
      );
      assert.equal(
        await assignment.getByLabel("Group", { exact: true }).inputValue(),
        "1",
      );
      await page.evaluate(() => {
        window.role = "viewer";
        window.project.lead_user_id = null;
        dispatchEvent(
          new CustomEvent("lific:realtime", {
            detail: { type: "project.updated", project_id: 4 },
          }),
        );
      });
      await page.waitForFunction(
        () => window.app.controller.state.role?.role === "viewer",
      );
      assert.equal(
        await page
          .getByRole("button", { name: "Download project archive" })
          .count(),
        0,
      );
      assert.equal(
        await page.locator("[data-project-identity] button").count(),
        0,
      );
      await page.evaluate(() => {
        window.session.state.user = { id: 2 };
        window.app.controller.accountChanged();
      });
      assert.equal(
        await page.locator("[data-project-settings-content]").textContent(),
        "",
      );
      assert.deepEqual(errors, []);
    } finally {
      await browser.close();
    }
  },
);
test(
  "headless archive import keeps uncertain outcome blocked until the project list is checked",
  { skip: !process.env.PLAYWRIGHT_EXECUTABLE_PATH },
  async () => {
    const { chromium } = await import(
      path.resolve(
        __dirname,
        "../../../..",
        "e2e/node_modules/playwright/index.mjs",
      )
    );
    const browser = await chromium.launch({
      headless: true,
      executablePath: process.env.PLAYWRIGHT_EXECUTABLE_PATH,
    });
    try {
      const page = await browser.newPage();
      page.setDefaultTimeout(5000);
      await page.route("http://lific.test/**", (route) =>
        route.fulfill({
          contentType: "text/html",
          body: '<html><head><title>Archive import</title></head><body><section data-topcoat-project-settings="archive"><p data-project-settings-status role="status"></p><p data-project-settings-error role="alert"></p><p data-project-settings-warning role="status"></p><div data-project-settings-content></div></section></body></html>',
        }),
      );
      await page.goto("http://lific.test/projects/import");
      await page.addScriptTag({
        content: fs.readFileSync(`${__dirname}/project-settings.js`, "utf8"),
      });
      await page.evaluate(() => {
        const session = {
          state: { publicProject: null, user: { id: 1 } },
          request: async (path) => ({
            ok: true,
            data:
              path === "/project-archives"
                ? { can_import: true, max_upload_bytes: 100 }
                : [],
          }),
        };
        window.app = LificTopcoatProjectSettings.attach(
          document.querySelector("[data-topcoat-project-settings]"),
          { session },
        );
        window.app.controller.env.upload = async (
          _file,
          progress,
          processing,
        ) => {
          progress(70);
          processing();
          return { ok: false, status: null, error: "Connection lost" };
        };
      });
      await page
        .getByLabel("Project archive (.tar.gz)", { exact: true })
        .setInputFiles({
          name: "project.tar.gz",
          mimeType: "application/gzip",
          buffer: Buffer.from("archive"),
        });
      await page.getByLabel(/I understand/).check();
      await page
        .getByRole("button", { name: "Import as private project" })
        .click();
      await page
        .getByRole("button", { name: "I've checked the project list" })
        .waitFor();
      assert.equal(
        await page
          .getByRole("button", { name: "Import as private project" })
          .count(),
        0,
      );
      await page
        .getByRole("button", { name: "I've checked the project list" })
        .click();
      await page
        .getByRole("button", { name: "Import as private project" })
        .waitFor();
    } finally {
      await browser.close();
    }
  },
);

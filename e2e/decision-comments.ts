import type { BrowserContext } from "playwright";

export async function checkDecisionComments(context: BrowserContext, base: string) {
  const page = await context.newPage();
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(String(error)));
  try {
    await page.goto(`${base}/DEMO/issues/DEMO-1`);
    const { issue, pageId } = await page.evaluate(async () => {
      const headers = {
        "Content-Type": "application/json",
      };
      async function request(path: string, body?: unknown) {
        const response = await fetch(`/api${path}`, {
          method: body === undefined ? "GET" : "POST",
          headers,
          body: body === undefined ? undefined : JSON.stringify(body),
        });
        if (!response.ok) throw new Error(`${path}: ${response.status} ${await response.text()}`);
        return response.json();
      }
      const projects = await request("/projects");
      const project = projects.find((item: { identifier: string }) => item.identifier === "DEMO");
      const issue = await request("/issues", { project_id: project.id, title: "Decision thread smoke" });
      await request(`/issues/${issue.id}/comments`, { content: "Old decision: use SQLite", kind: "decision" });
      for (let i = 0; i < 52; i++) {
        await request(`/issues/${issue.id}/comments`, { content: `Discussion note ${i}` });
      }
      // A page of its own: the project's first page is its conventions page,
      // which opens on the Conventions tab.
      const doc = await request("/pages", { project_id: project.id, title: "Decision page smoke" });
      return { issue, pageId: doc.id as number };
    });
    await page.goto(`${base}/DEMO/issues/${issue.identifier}`);
    const thread = page.locator("section.cmt");
    await thread.getByText("Discussion note 51", { exact: true }).waitFor();
    if (await thread.getByText("Old decision: use SQLite", { exact: true }).count()) {
      throw new Error("Old decision should start outside the newest comment window");
    }
    await thread.getByLabel("Decisions only", { exact: true }).check();
    await thread.getByText("Old decision: use SQLite", { exact: true }).waitFor();
    await thread.locator(".cmt__kind").getByText("Decision", { exact: true }).waitFor();
    if (await thread.getByText("Discussion note 51", { exact: true }).count()) {
      throw new Error("Decisions filter left discussion comments visible");
    }
    console.log("ok   decision filter finds an older decision beyond the newest page and badges it");

    await thread.getByLabel("Record as decision", { exact: true }).check();
    await thread.locator("textarea").fill("New decision: keep the public API");
    await thread.getByRole("button", { name: "Comment", exact: true }).click();
    const added = thread.locator(".cmt__item").filter({ hasText: "New decision: keep the public API" });
    await added.locator(".cmt__kind").getByText("Decision", { exact: true }).waitFor();
    if (await thread.getByLabel("Record as decision", { exact: true }).isChecked()) {
      throw new Error("A fresh composer should default back to an ordinary comment");
    }
    await page.reload();
    await thread.getByText("New decision: keep the public API", { exact: true }).waitFor();
    await thread.getByLabel("Decisions only", { exact: true }).check();
    await thread.getByText("Old decision: use SQLite", { exact: true }).waitFor();
    if (await thread.locator(".cmt__item").count() !== 2) throw new Error("Expected both saved decisions after reload");
    await thread.getByLabel("Decisions only", { exact: true }).uncheck();
    await thread.getByText("Discussion note 51", { exact: true }).waitFor();
    console.log("ok   browser composer saves a decision and filter toggles back to discussion");

    await page.goto(`${base}/DEMO/pages/${pageId}`);
    await thread.getByLabel("Record as decision", { exact: true }).check();
    await thread.locator("textarea").fill("Page decision: document the protocol");
    await thread.getByRole("button", { name: "Comment", exact: true }).click();
    await thread.locator(".cmt__item").filter({ hasText: "Page decision: document the protocol" })
      .locator(".cmt__kind").getByText("Decision", { exact: true }).waitFor();
    await thread.getByLabel("Record as decision", { exact: true }).uncheck();
    await thread.locator("textarea").fill("Page discussion note");
    await thread.getByRole("button", { name: "Comment", exact: true }).click();
    await thread.getByText("Page discussion note", { exact: true }).waitFor();
    await thread.getByLabel("Decisions only", { exact: true }).check();
    await thread.getByText("Page decision: document the protocol", { exact: true }).waitFor();
    if (await thread.getByText("Page discussion note", { exact: true }).count()) {
      throw new Error("Page decision filter left discussion comments visible");
    }
    console.log("ok   page comments also save, badge and filter decisions");
    if (errors.length) throw new Error(errors.join("; "));
  } finally {
    await page.close();
  }
}

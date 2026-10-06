import type { BrowserContext, Page } from "playwright";

async function createIssues(page: Page, titles: string[]): Promise<string[]> {
  return page.evaluate(async (titles) => {
    const headers = {
      Authorization: `Bearer ${localStorage.getItem("lific_token")}`,
      "Content-Type": "application/json",
    };
    const request = async (path: string, body?: unknown) => {
      const res = await fetch(`/api${path}`, {
        method: body === undefined ? "GET" : "POST", headers,
        body: body === undefined ? undefined : JSON.stringify(body),
      });
      if (!res.ok) throw new Error(`${path}: ${res.status} ${await res.text()}`);
      return res.json();
    };
    const projects = await request("/projects");
    const project = projects.find((p: { identifier: string }) => p.identifier === "DEMO");
    const created: string[] = [];
    for (const title of titles) {
      created.push((await request("/issues", { project_id: project.id, title })).identifier);
    }
    // Two `relates_to` rows in opposite directions: the issue view must list
    // the other issue once rather than fail on the repeated identifier.
    await request("/issues/link", { source: created[2], target: created[3], relation_type: "relates_to" });
    await request("/issues/link", { source: created[3], target: created[2], relation_type: "relates_to" });
    return created;
  }, titles);
}

function relationField(page: Page, label: string) {
  return page.locator(".issue-meta-field").filter({
    has: page.locator(".issue-meta-field-label", { hasText: new RegExp(`^${label}$`, "i") }),
  });
}

export async function checkRelationEditing(context: BrowserContext, base: string) {
  const page = await context.newPage();
  const pageErrors: string[] = [];
  page.on("pageerror", (err) => pageErrors.push(String(err)));
  try {
    await page.goto(`${base}/DEMO/issues/DEMO-1`);
    const [blocked, blocker, mirroredA, mirroredB] = await createIssues(page, [
      "Blocked work",
      "Prerequisite",
      "Mirrored A",
      "Mirrored B",
    ]);

    // Add "Blocked by" from the issue view: the other issue becomes the source.
    await page.goto(`${base}/DEMO/issues/${blocked}`);
    await page.getByRole("button", { name: "Add relation" }).click();
    await page.locator(".issue-meta-relations").getByRole("button", { name: "Blocked by", exact: true }).click();
    await page.keyboard.type(blocker);
    // The typed identifier resolves to the first hit; click it rather than
    // racing the debounced search with Enter.
    await page.locator('[data-idx="0"]', { hasText: "Prerequisite" }).click();
    await relationField(page, "Blocked by")
      .getByRole("button", { name: blocker, exact: true })
      .waitFor({ state: "visible" });

    await page.goto(`${base}/DEMO/issues/${blocker}`);
    await relationField(page, "Blocks")
      .getByRole("button", { name: blocked, exact: true })
      .waitFor({ state: "visible" });

    // Removing the chip unlinks the pair, and the empty group disappears.
    await page.getByRole("button", { name: `Remove relation with ${blocked}` }).click();
    await relationField(page, "Blocks").waitFor({ state: "detached" });

    await page.goto(`${base}/DEMO/issues/${mirroredA}`);
    const related = relationField(page, "Related").getByRole("button", { name: mirroredB, exact: true });
    await related.waitFor({ state: "visible" });
    const count = await related.count();
    if (count !== 1) throw new Error(`mirrored relation listed ${count} times`);
    if (pageErrors.length > 0) throw new Error(`uncaught page errors: ${pageErrors.join("; ")}`);
    console.log("ok   relations are added and removed from the issue view, mirrored ones listed once");
  } finally {
    await page.close();
  }
}

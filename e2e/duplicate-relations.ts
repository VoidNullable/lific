import type { BrowserContext } from "playwright";

export async function checkDuplicateRelations(context: BrowserContext, base: string) {
  const page = await context.newPage();
  try {
    await page.goto(`${base}/DEMO/issues/DEMO-1`);
    const [source, target] = await page.evaluate(async () => {
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
      const source = await request("/issues", { project_id: project.id, title: "Duplicate source" });
      const target = await request("/issues", { project_id: project.id, title: "Canonical target" });
      await request("/issues/link", { source: source.identifier, target: target.identifier, relation_type: "duplicate" });
      return [source.identifier, target.identifier];
    });
    await page.goto(`${base}/DEMO/issues/${source}`);
    const forward = page.locator(".issue-meta-field").filter({ hasText: "Duplicate of" });
    await forward.getByRole("button", { name: target, exact: true }).waitFor({ state: "visible" });
    await forward.getByRole("button", { name: target, exact: true }).click();
    await page.waitForURL(new RegExp(`${target}$`));
    const reverse = page.locator(".issue-meta-field").filter({ hasText: "Duplicated by" });
    await reverse.getByRole("button", { name: source, exact: true }).waitFor({ state: "visible" });
    await page.setViewportSize({ width: 390, height: 844 });
    await reverse.getByRole("button", { name: source, exact: true }).waitFor({ state: "visible" });
    console.log("ok   duplicate-only relations render both directions on desktop and mobile");
  } finally {
    await page.close();
  }
}

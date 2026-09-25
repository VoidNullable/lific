import type { BrowserContext } from "playwright";

/** `YYYY-MM-DD` for the local day `offset` days from today. */
function day(offset: number): string {
  const date = new Date();
  date.setDate(date.getDate() + offset);
  const m = String(date.getMonth() + 1).padStart(2, "0");
  const d = String(date.getDate()).padStart(2, "0");
  return `${date.getFullYear()}-${m}-${d}`;
}

/**
 * LIF-485: add a user blocker and a date blocker from the issue page, then
 * find them on the detail view, the issue list and the board. The date
 * window ended yesterday, so it must render as overdue everywhere.
 */
export async function checkIssueWaits(context: BrowserContext, base: string, username: string) {
  const page = await context.newPage();
  const errors: string[] = [];
  page.on("pageerror", (err) => errors.push(String(err)));
  try {
    await page.goto(`${base}/DEMO/issues/DEMO-1`);
    const identifier = await page.evaluate(async () => {
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
      const issue = await request("/issues", {
        project_id: project.id,
        title: "Waiting smoke issue",
        status: "todo",
      });
      return issue.identifier as string;
    });

    await page.goto(`${base}/DEMO/issues/${identifier}`);
    const field = page.getByTestId("issue-waits");
    const form = field.getByRole("form", { name: "Add blocker" });

    // A person.
    await field.getByRole("button", { name: "Add blocker" }).click();
    const person = form.getByLabel("Person");
    await person.locator(`option[value="${username}"]`).waitFor({ state: "attached" });
    await person.selectOption(username);
    await form.getByLabel("Note").fill("Decide the scope");
    await form.getByRole("button", { name: "Add", exact: true }).click();
    const userItem = field.locator('li[data-wait-kind="user"]');
    await userItem.filter({ hasText: `@${username}` }).waitFor({ state: "visible" });
    await userItem.filter({ hasText: "Decide the scope" }).waitFor({ state: "visible" });

    // A window of days that closed yesterday.
    await field.getByRole("button", { name: "Add blocker" }).click();
    await form.getByRole("button", { name: "Dates" }).click();
    await form.getByLabel("From").fill(day(-4));
    await form.getByLabel("Until (optional)").fill(day(-1));
    await form.getByLabel("Note").fill("State filing office");
    await form.getByRole("button", { name: "Add", exact: true }).click();
    const dateItem = field.locator('li[data-wait-kind="date"][data-wait-state="overdue"]');
    await dateItem.filter({ hasText: "Overdue since" }).waitFor({ state: "visible" });
    await dateItem.filter({ hasText: "State filing office" }).waitFor({ state: "visible" });
    console.log("ok   issue detail adds and shows user and date blockers (LIF-485)");

    // The list row names the most pressing wait and carries every one.
    await page.goto(`${base}/DEMO/issues`);
    const row = page.getByRole("group", { name: identifier, exact: true });
    const rowChip = row.locator('.wait-chip[data-wait-state="overdue"]');
    await rowChip.waitFor({ state: "visible", timeout: 15_000 });
    const rowLabel = (await rowChip.getAttribute("aria-label")) ?? "";
    if (!rowLabel.includes(`@${username}`) || !rowLabel.includes("Overdue since")) {
      throw new Error(`list chip does not name both waits: ${JSON.stringify(rowLabel)}`);
    }
    console.log("ok   issue list shows the waiting indicator, overdue distinct (LIF-485)");

    await page.goto(`${base}/DEMO/board`);
    const card = page.locator("article").filter({ hasText: "Waiting smoke issue" });
    const cardChip = card.locator('.wait-chip[data-wait-state="overdue"]');
    await cardChip.waitFor({ state: "visible", timeout: 15_000 });
    const cardLabel = (await cardChip.getAttribute("aria-label")) ?? "";
    if (!cardLabel.includes(`@${username}`)) {
      throw new Error(`board chip does not name the person: ${JSON.stringify(cardLabel)}`);
    }
    console.log("ok   board card shows the waiting indicator (LIF-485)");

    // Clearing the person leaves only the date blocker.
    await page.goto(`${base}/DEMO/issues/${identifier}`);
    await field
      .getByRole("button", { name: new RegExp(`^Clear blocker: Waiting on .*@${username}`) })
      .click();
    await field.locator('li[data-wait-kind="user"]').waitFor({ state: "detached" });
    await field.locator('li[data-wait-kind="date"]').waitFor({ state: "visible" });
    console.log("ok   issue detail clears a user blocker (LIF-485)");

    if (errors.length > 0) throw new Error(`page errors: ${errors.join("; ")}`);
  } finally {
    await page.close();
  }
}

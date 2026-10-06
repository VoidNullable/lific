import type { BrowserContext, Page, Route } from "playwright";

/** Calls the REST API from inside the page, with the session's token. */
async function api(page: Page, path: string, body?: unknown): Promise<any> {
  return page.evaluate(
    async ([path, body]) => {
      const res = await fetch(`/api${path}`, {
        method: body === undefined ? "GET" : "POST",
        headers: {
          Authorization: `Bearer ${localStorage.getItem("lific_token")}`,
          "Content-Type": "application/json",
        },
        body: body === undefined ? undefined : JSON.stringify(body),
      });
      if (!res.ok) throw new Error(`${path}: ${res.status} ${await res.text()}`);
      return res.json();
    },
    [path, body] as const,
  );
}

async function createIssues(page: Page, titles: string[]): Promise<string[]> {
  const projects = await api(page, "/projects");
  const project = projects.find((p: { identifier: string }) => p.identifier === "DEMO");
  const created: string[] = [];
  for (const title of titles) {
    created.push((await api(page, "/issues", { project_id: project.id, title })).identifier);
  }
  return created;
}

function link(page: Page, source: string, target: string, relation_type: string) {
  return api(page, "/issues/link", { source, target, relation_type });
}

function relationField(page: Page, label: string) {
  return page.locator(".issue-meta-field").filter({
    has: page.locator(".issue-meta-field-label", { hasText: new RegExp(`^${label}$`, "i") }),
  });
}

/** Sends the first request matching `url` to the server right away, but holds
 *  its response until `release()`: the page then receives an answer that was
 *  true when asked and may be stale by the time it lands. `captured` resolves
 *  with that answer's body once the server has produced it. */
async function holdFirst(page: Page, url: string | RegExp) {
  let release!: () => void;
  const gate = new Promise<void>((resolve) => (release = resolve));
  let capture!: (body: any) => void;
  const captured = new Promise<any>((resolve) => (capture = resolve));
  let delivered: Promise<void> | null = null;
  const handler = async (route: Route) => {
    if (delivered) return route.continue();
    delivered = route.fetch().then(async (response) => {
      capture(await response.json());
      await gate;
      await route.fulfill({ response });
    });
    await delivered;
  };
  await page.route(url, handler);
  return {
    captured,
    release: async () => {
      release();
      // Unrouting before the held response is delivered makes Playwright
      // report the route as handled twice.
      await delivered;
      await page.unroute(url, handler);
    },
  };
}

async function openAddRelation(page: Page, kind: string) {
  await page.getByRole("button", { name: "Add relation" }).click();
  await page.locator(".issue-meta-relations").getByRole("button", { name: kind, exact: true }).click();
}

export async function checkRelationEditing(context: BrowserContext, base: string) {
  const page = await context.newPage();
  const pageErrors: string[] = [];
  page.on("pageerror", (err) => pageErrors.push(String(err)));
  try {
    await page.goto(`${base}/DEMO/issues/DEMO-1`);
    const [blocked, blocker, mirroredA, mirroredB, origin, elsewhere, picked, many, first, second] =
      await createIssues(page, [
        "Blocked work",
        "Prerequisite",
        "Mirrored A",
        "Mirrored B",
        "Picker origin",
        "Picker elsewhere",
        "Picker target",
        "Many relations",
        "First related",
        "Second related",
      ]);
    // Two `relates_to` rows in opposite directions: the issue view must list
    // the other issue once rather than fail on the repeated identifier.
    await link(page, mirroredA, mirroredB, "relates_to");
    await link(page, mirroredB, mirroredA, "relates_to");
    await link(page, elsewhere, origin, "relates_to");
    await link(page, many, first, "relates_to");
    await link(page, many, second, "relates_to");

    // Add "Blocked by" from the issue view: the other issue becomes the source.
    await page.goto(`${base}/DEMO/issues/${blocked}`);
    await openAddRelation(page, "Blocked by");
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

    // A picker selection that resolves after the reader navigated away must
    // not be linked to the issue now on screen.
    await page.goto(`${base}/DEMO/issues/${elsewhere}`);
    await relationField(page, "Related").getByRole("button", { name: origin, exact: true }).click();
    await page.waitForURL(new RegExp(`${origin}$`));
    await openAddRelation(page, "Blocks");
    await page.keyboard.type(picked);
    const hit = page.locator('[data-idx="0"]', { hasText: "Picker target" });
    await hit.waitFor({ state: "visible" });
    const pick = await holdFirst(page, new RegExp(`/api/issues/resolve/${picked}$`));
    await hit.click();
    await page.goBack();
    await page.waitForURL(new RegExp(`${elsewhere}$`));
    await relationField(page, "Related").getByRole("button", { name: origin, exact: true }).waitFor();
    // A picker opened on the new issue must not adopt the late selection, nor
    // be closed by it.
    await openAddRelation(page, "Related");
    const search = page.getByPlaceholder(/Search DEMO issues/);
    await search.waitFor({ state: "visible" });
    await pick.release();
    await page.waitForTimeout(1000);
    if (!(await search.isVisible())) throw new Error("a late picker selection closed a newer picker");
    await page.keyboard.press("Escape");
    for (const id of [origin, elsewhere]) {
      const issue = await api(page, `/issues/resolve/${id}`);
      if (issue.blocks?.length) throw new Error(`late picker selection linked ${id} blocks ${issue.blocks}`);
      if (issue.relates_to?.includes(picked)) throw new Error(`late picker selection linked ${id} to ${picked}`);
    }

    // Two quick removals: the reload after the first lands last and must not
    // paint the relation the second removal already took away.
    await page.goto(`${base}/DEMO/issues/${many}`);
    await relationField(page, "Related").getByRole("button", { name: second, exact: true }).waitFor();
    const reload = await holdFirst(page, new RegExp(`/api/issues/resolve/${many}$`));
    const unlinked = page.waitForResponse((r) => r.url().endsWith("/api/issues/unlink"));
    await page.getByRole("button", { name: `Remove relation with ${first}` }).click();
    await unlinked;
    // Only a snapshot taken before the second removal can be stale.
    const snapshot = await reload.captured;
    if (!snapshot.relates_to?.includes(second)) {
      throw new Error(`held reload missed ${second}; the race was not set up`);
    }
    await page.getByRole("button", { name: `Remove relation with ${second}` }).click();
    await relationField(page, "Related").waitFor({ state: "detached" });
    await reload.release();
    // A later refresh would correct a stale paint, so watch for the removed
    // relations reappearing at any point rather than checking the end state.
    const repainted = await relationField(page, "Related")
      .waitFor({ state: "attached", timeout: 1500 })
      .then(
        () => true,
        (err: Error) => {
          if (err.name !== "TimeoutError") throw err;
          return false;
        },
      );
    if (repainted) throw new Error("a superseded reload repainted removed relations");

    if (pageErrors.length > 0) throw new Error(`uncaught page errors: ${pageErrors.join("; ")}`);
    console.log("ok   relations are added and removed from the issue view, mirrored ones listed once");
    console.log("ok   late picker selections and superseded relation reloads are dropped");
  } finally {
    await page.close();
  }
}

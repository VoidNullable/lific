import type { Browser, Page } from "playwright";

async function signIn(page: Page, username: string, password: string) {
  await page.locator("#login-identity").waitFor({ state: "visible", timeout: 15_000 });
  await page.fill("#login-identity", username);
  await page.fill("#login-password", password);
  await page.click("button[type=submit]");
}

/**
 * A link opened without a session goes through the login screen and comes
 * back to the page it named, instead of landing on Home. Each case runs in a
 * fresh context, so there is no session to begin with.
 */
export async function checkLoginRedirect(
  browser: Browser,
  base: string,
  username: string,
  password: string,
) {
  const cases: { open: string; lands: RegExp; expect: string }[] = [
    // Path-style deep link, translated to a hash route at boot.
    { open: "/DEMO/issues/DEMO-1", lands: /#\/DEMO\/issues\/DEMO-1$/, expect: "First smoke comment" },
    // Hash route with its own query string.
    {
      open: "/#/DEMO/issues/new?status=todo",
      lands: /#\/DEMO\/issues\/new\?status=todo$/,
      expect: "Create issue",
    },
  ];

  for (const c of cases) {
    const context = await browser.newContext();
    const page = await context.newPage();
    const errors: string[] = [];
    page.on("pageerror", (err) => errors.push(String(err)));
    try {
      await page.goto(`${base}${c.open}`, { waitUntil: "load", timeout: 15_000 });
      await page.waitForURL(/#\/login\?next=/, { timeout: 15_000 });
      const entriesBefore = await page.evaluate(() => history.length);
      await signIn(page, username, password);
      await page.waitForURL(c.lands, { timeout: 15_000 });
      await page.getByText(c.expect).first().waitFor({ state: "visible", timeout: 15_000 });

      // The login screen replaced its own history entry, so Back does not
      // return to the form (which would only bounce forward again).
      const entriesAfter = await page.evaluate(() => history.length);
      if (entriesAfter !== entriesBefore) {
        throw new Error(`${c.open}: sign-in added ${entriesAfter - entriesBefore} history entries`);
      }
      if (errors.length > 0) throw new Error(`${c.open}: page errors: ${errors.join("; ")}`);
      console.log(`ok   login returns to ${c.open}`);
    } finally {
      await context.close();
    }
  }

  // A crafted next cannot send anyone off the app; it falls back to Home.
  {
    const context = await browser.newContext();
    const page = await context.newPage();
    try {
      await page.goto(`${base}/#/login?next=${encodeURIComponent("//evil.example/x")}`, {
        waitUntil: "load",
        timeout: 15_000,
      });
      await signIn(page, username, password);
      await page.waitForURL(`${base}/#/`, { timeout: 15_000 });
      console.log("ok   login ignores an off-app next");
    } finally {
      await context.close();
    }
  }

  // Signed in already: opening a remembered login link goes straight on.
  {
    const context = await browser.newContext();
    const page = await context.newPage();
    try {
      await page.goto(`${base}/#/login`, { waitUntil: "load", timeout: 15_000 });
      await signIn(page, username, password);
      await page.waitForURL(`${base}/#/`, { timeout: 15_000 });
      const remembered = `${base}/#/login?next=${encodeURIComponent("/DEMO/pages/1")}`;
      // A fresh page, so this is a cold load of the login link, not a hash
      // change inside an already-running app.
      const fresh = await context.newPage();
      await fresh.goto(remembered, { waitUntil: "load", timeout: 15_000 });
      await fresh.waitForURL(/#\/DEMO\/pages\/1$/, { timeout: 15_000 }).catch(() => {
        throw new Error(`signed-in login link stayed at ${fresh.url()}`);
      });
      console.log("ok   a signed-in login link continues to next");
    } finally {
      await context.close();
    }
  }
}

import { describe, expect, it } from "vitest";
import { pageTitle, routePageTitle } from "../src/lib/pageTitle";

describe("pageTitle", () => {
  it("joins parts with the separator and closes with the app name", () => {
    expect(pageTitle("Issues", "NEWS")).toBe("Issues · NEWS · Lific");
  });

  it("drops falsy and whitespace-only parts", () => {
    expect(pageTitle("Home", null, undefined, false, "  ")).toBe("Home · Lific");
  });

  it("renders a single part as itself plus the app name", () => {
    expect(pageTitle("Sign in")).toBe("Sign in · Lific");
  });

  it("caps an oversized part so the app name stays visible", () => {
    const long = "X".repeat(200);
    const title = pageTitle("NEWS-18", long, "NEWS");
    expect(title.endsWith("· Lific")).toBe(true);
    const parts = title.split(" · ");
    expect(parts[0]).toBe("NEWS-18");
    expect(parts[1]!.length).toBeLessThanOrEqual(61);
  });

  it("trims parts", () => {
    expect(pageTitle("  Home  ")).toBe("Home · Lific");
  });
});

describe("routePageTitle", () => {
  it("maps auth pages", () => {
    expect(routePageTitle({ type: "auth", page: "login" })).toBe("Sign in · Lific");
    expect(routePageTitle({ type: "auth", page: "signup" })).toBe("Create account · Lific");
  });

  it("maps top-level pages without a project", () => {
    expect(routePageTitle({ type: "app", page: "home" })).toBe("Home · Lific");
    expect(routePageTitle({ type: "app", page: "settings" })).toBe("Settings · Lific");
    expect(routePageTitle({ type: "app", page: "instance-settings" })).toBe("Instance settings · Lific");
    expect(routePageTitle({ type: "app", page: "project-new" })).toBe("New project · Lific");
  });

  it("maps project sections as section then project", () => {
    expect(routePageTitle({ type: "app", page: "issues", project: "NEWS" })).toBe("Issues · NEWS · Lific");
    expect(routePageTitle({ type: "app", page: "board", project: "NEWS" })).toBe("Board · NEWS · Lific");
    expect(routePageTitle({ type: "app", page: "pages", project: "WP" })).toBe("Pages · WP · Lific");
    expect(routePageTitle({ type: "app", page: "activity", project: "WP" })).toBe("Activity · WP · Lific");
  });

  it("maps detail fallbacks to the identifier, not a fake title", () => {
    expect(routePageTitle({ type: "app", page: "issue-detail", project: "NEWS", identifier: "NEWS-18" }))
      .toBe("NEWS-18 · NEWS · Lific");
    expect(routePageTitle({ type: "app", page: "page-detail", project: "NEWS" })).toBe("Page · NEWS · Lific");
    expect(routePageTitle({ type: "app", page: "module-detail", project: "NEWS" })).toBe("Module · NEWS · Lific");
  });

  it("resolves a public route to its inner page", () => {
    expect(routePageTitle({ type: "public", project: "NEWS", inner: { type: "app", page: "issue-detail", project: "NEWS", identifier: "NEWS-18" } }))
      .toBe("NEWS-18 · NEWS · Lific");
  });

  it("maps not-found and degrades unknown pages to the bare name", () => {
    expect(routePageTitle({ type: "not-found" })).toBe("Not found · Lific");
    expect(routePageTitle({ type: "app", page: "something-new" })).toBe("Lific");
    expect(routePageTitle({ type: "loading" })).toBe("Lific");
  });
});

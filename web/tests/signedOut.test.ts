import { afterEach, describe, expect, test } from "bun:test";
import {
  SIGNED_OUT_PAGES,
  currentRoute,
  isSignedOut,
  onSignedOutChange,
  rewritePublicRoute,
  setSignedOut,
  signInRoute,
} from "../src/lib/signedOut";

// LIF-583: a published project is read at its normal links by anyone. These
// pin the one piece of state the components consult, the rewrite of the old
// public-view routes, and the login route a signed-out reader is sent to.

afterEach(() => setSignedOut(false));

describe("signed-out state", () => {
  test("is off by default and notifies listeners on transitions only", () => {
    expect(isSignedOut()).toBe(false);
    const seen: boolean[] = [];
    const off = onSignedOutChange((v) => seen.push(v));
    setSignedOut(true);
    setSignedOut(true);
    setSignedOut(false);
    off();
    setSignedOut(true);
    expect(seen).toEqual([true, false]);
    expect(isSignedOut()).toBe(true);
  });

  test("covers a project's read pages and nothing account-shaped", () => {
    for (const page of ["project-overview", "issues", "board", "graph", "issue-detail", "pages",
      "page-detail", "plans", "plan-detail", "modules", "module-detail", "activity", "insights",
      "files"]) {
      expect(SIGNED_OUT_PAGES.has(page)).toBe(true);
    }
    for (const page of ["home", "inbox", "assigned", "agents", "settings", "instance-settings",
      "project-new", "project-import", "issue-new", "project-settings"]) {
      expect(SIGNED_OUT_PAGES.has(page)).toBe(false);
    }
  });
});

describe("rewritePublicRoute", () => {
  test("sends the bare project to its issue list", () => {
    expect(rewritePublicRoute("/public/LIF")).toBe("/LIF/issues");
    expect(rewritePublicRoute("/public/LIF/")).toBe("/LIF/issues");
  });

  test("sends the short issue form to the issue", () => {
    expect(rewritePublicRoute("/public/LIF/LIF-42")).toBe("/LIF/issues/LIF-42");
  });

  test("drops the prefix from every other route and keeps the query", () => {
    expect(rewritePublicRoute("/public/LIF/issues/LIF-42")).toBe("/LIF/issues/LIF-42");
    expect(rewritePublicRoute("/public/LIF/pages/7")).toBe("/LIF/pages/7");
    expect(rewritePublicRoute("/public/LIF/board")).toBe("/LIF/board");
    expect(rewritePublicRoute("/public/LIF/issues/LIF-42?comment=9")).toBe(
      "/LIF/issues/LIF-42?comment=9",
    );
  });

  test("leaves ordinary routes alone", () => {
    expect(rewritePublicRoute("/LIF/issues")).toBeNull();
    expect(rewritePublicRoute("/")).toBeNull();
    expect(rewritePublicRoute("/public")).toBeNull();
    expect(rewritePublicRoute("/publicity/issues")).toBeNull();
  });
});

describe("signInRoute", () => {
  test("returns to the hash route being read", () => {
    const loc = { pathname: "/", hash: "#/LIF/issues/LIF-42" };
    expect(currentRoute(loc)).toBe("/LIF/issues/LIF-42");
    expect(signInRoute(loc)).toBe("/login?next=%2FLIF%2Fissues%2FLIF-42");
  });

  test("folds a path-style comment link back into the route", () => {
    const loc = { pathname: "/LIF/issues/LIF-42", hash: "#comment-7" };
    expect(currentRoute(loc)).toBe("/LIF/issues/LIF-42?comment=7");
    expect(signInRoute(loc)).toBe(
      `/login?${new URLSearchParams({ next: "/LIF/issues/LIF-42?comment=7" })}`,
    );
  });

  test("remembers nothing for the root", () => {
    expect(signInRoute({ pathname: "/", hash: "" })).toBe("/login");
  });
});

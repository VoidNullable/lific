import { describe, expect, test } from "bun:test";
import {
  authRouteFor,
  loginRouteFor,
  nextRouteFrom,
  safeNextRoute,
} from "../src/lib/loginRedirect";

describe("login redirect", () => {
  test("remembers the page a signed-out visitor opened", () => {
    expect(loginRouteFor("/LIF/issues/LIF-42")).toBe("/login?next=%2FLIF%2Fissues%2FLIF-42");
  });

  test("round-trips a route that carries its own query", () => {
    const login = loginRouteFor("/LIF/issues/new?module=3&status=todo");
    // The inner query is encoded, so the router still sees `/login` as the path.
    expect(login.split("?")[0]).toBe("/login");
    expect(login.split("?").length).toBe(2);
    expect(nextRouteFrom(login)).toBe("/LIF/issues/new?module=3&status=todo");
  });

  test("keeps a comment target through sign-in", () => {
    expect(nextRouteFrom(loginRouteFor("/LIF/issues/LIF-42?comment=7"))).toBe(
      "/LIF/issues/LIF-42?comment=7",
    );
  });

  test("does not remember Home or the auth screens", () => {
    expect(loginRouteFor("/")).toBe("/login");
    expect(loginRouteFor("")).toBe("/login");
    expect(loginRouteFor(null)).toBe("/login");
    expect(loginRouteFor("/signup")).toBe("/login");
  });

  test("a second bounce from an auth screen keeps the first next", () => {
    expect(loginRouteFor("/login?next=%2FLIF%2Fissues")).toBe("/login?next=%2FLIF%2Fissues");
    expect(authRouteFor("login", "/signup?next=%2FLIF%2Fpages%2F7")).toBe(
      "/login?next=%2FLIF%2Fpages%2F7",
    );
    expect(loginRouteFor("/login?next=%2F%2Fevil.example")).toBe("/login");
  });

  test("carries next between sign in and sign up", () => {
    expect(authRouteFor("signup", "/LIF/pages/7")).toBe("/signup?next=%2FLIF%2Fpages%2F7");
    expect(authRouteFor("login", "/")).toBe("/login");
  });

  test("falls back to Home without a usable next", () => {
    expect(nextRouteFrom("/login")).toBe("/");
    expect(nextRouteFrom("/login?next=")).toBe("/");
    expect(nextRouteFrom("/login?other=1")).toBe("/");
  });

  test("refuses anything that is not an in-app route", () => {
    for (const next of [
      "https://evil.example",
      "//evil.example",
      "/\\evil.example",
      "javascript:alert(1)",
      "LIF/issues",
      "/LIF\n/issues",
      "/login",
      "/signup?next=%2F",
      "/" + "a".repeat(3000),
    ]) {
      expect(safeNextRoute(next)).toBeNull();
      expect(nextRouteFrom(`/login?${new URLSearchParams({ next })}`)).toBe("/");
    }
  });
});

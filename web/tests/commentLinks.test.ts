import { describe, expect, test } from "bun:test";
import {
  commentTargetFromHash,
  routeForCommentHash,
  routeWithCommentTarget,
  splitResourcePath,
} from "../src/lib/commentLinks";

describe("comment links", () => {
  test("keeps a public prefix inside the route rather than the base path", () => {
    expect(splitResourcePath("/public/LIF/issues/LIF-42")).toEqual({
      basePath: "",
      route: "/public/LIF/issues/LIF-42",
    });
    expect(splitResourcePath("/app/public/LIF/pages/7")).toEqual({
      basePath: "/app",
      route: "/public/LIF/pages/7",
    });
    expect(splitResourcePath("/LIF/issues/LIF-42")).toEqual({
      basePath: "",
      route: "/LIF/issues/LIF-42",
    });
  });

  test("reads canonical anchors and normalized hash-route queries", () => {
    expect(commentTargetFromHash("#comment-42")).toBe("comment-42");
    expect(
      commentTargetFromHash("#/LIF/issues/LIF-1?view=all&comment=42"),
    ).toBe("comment-42");
    expect(commentTargetFromHash("#unrelated")).toBeNull();
  });

  test("preserves route query parameters while carrying the comment target", () => {
    expect(
      routeWithCommentTarget("/LIF/issues/LIF-1?view=all", "comment-42"),
    ).toBe("/LIF/issues/LIF-1?view=all&comment=42");
  });

  test("keeps plain anchors on the current route and adopts routed hashes", () => {
    expect(
      routeForCommentHash("#comment-42", "/LIF/issues/LIF-1"),
    ).toBe("/LIF/issues/LIF-1");
    expect(
      routeForCommentHash(
        "#/LIF/issues/LIF-2?comment=42",
        "/LIF/issues/LIF-1",
      ),
    ).toBe("/LIF/issues/LIF-2?comment=42");
  });

  test("separates a public URL base path from the resource route", () => {
    expect(splitResourcePath("/lific/LIF/issues/LIF-42")).toEqual({
      basePath: "/lific",
      route: "/LIF/issues/LIF-42",
    });
    expect(splitResourcePath("/LIF/pages/17")).toEqual({
      basePath: "",
      route: "/LIF/pages/17",
    });
    expect(splitResourcePath("/unrelated/path")).toBeNull();
  });
});

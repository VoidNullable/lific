import { afterEach, describe, expect, test } from "bun:test";
import {
  getPublicProject,
  onPublicScopeChange,
  publicMirror,
  publicSynthetic,
  scopedRoute,
  setPublicProject,
} from "../src/lib/publicScope";

// LIF-471: the client half of the public boundary. Every request from a
// public page goes through `publicMirror` / `publicSynthetic`; a path neither
// answers is refused by api.ts. These pin the mirror rule so a new private
// endpoint cannot silently gain a public twin, and so the ones the public
// components need keep theirs.

afterEach(() => setPublicProject(null));

describe("public scope", () => {
  test("is off by default and uppercases the project it is set to", () => {
    expect(getPublicProject()).toBeNull();
    expect(publicMirror("/projects")).toBeNull();
    setPublicProject("lif");
    expect(getPublicProject()).toBe("LIF");
  });

  test("notifies listeners on transitions only", () => {
    const seen: (string | null)[] = [];
    const off = onPublicScopeChange((p) => seen.push(p));
    setPublicProject("LIF");
    setPublicProject("LIF");
    setPublicProject(null);
    off();
    setPublicProject("LIF");
    expect(seen).toEqual(["LIF", null]);
  });

  test("prefixes in-app routes only while set", () => {
    expect(scopedRoute("/LIF/issues/LIF-1")).toBe("/LIF/issues/LIF-1");
    setPublicProject("LIF");
    expect(scopedRoute("/LIF/issues/LIF-1")).toBe("/public/LIF/issues/LIF-1");
    expect(scopedRoute("/public/LIF/pages")).toBe("/public/LIF/pages");
  });
});

describe("the mirror rule", () => {
  const base = "/public/api/projects/LIF";

  test("maps every read the public components make", () => {
    setPublicProject("LIF");
    expect(publicMirror("/projects")).toBe(base);
    expect(publicMirror("/projects/3/index")).toBe(`${base}/index`);
    expect(publicMirror("/projects/3/changes?since=42&limit=100")).toBe(
      `${base}/changes?since=42&limit=100`,
    );
    expect(publicMirror("/modules?project_id=3")).toBe(`${base}/modules`);
    expect(publicMirror("/labels?project_id=3")).toBe(`${base}/labels`);
    expect(publicMirror("/folders?project_id=3")).toBe(`${base}/folders`);
    expect(publicMirror("/issues/resolve/LIF-42")).toBe(`${base}/issues/resolve/LIF-42`);
    expect(publicMirror("/issues/9")).toBe(`${base}/issues/9`);
    expect(publicMirror("/issues/9/comments?order=desc&limit=50")).toBe(
      `${base}/issues/9/comments?order=desc&limit=50`,
    );
    expect(publicMirror("/pages/7")).toBe(`${base}/pages/7`);
    expect(publicMirror("/pages/7/comments")).toBe(`${base}/pages/7/comments`);
    expect(publicMirror("/attachments?entity_type=issue&entity_id=9")).toBe(
      `${base}/attachments?entity_type=issue&entity_id=9`,
    );
    expect(publicMirror("/attachments/5")).toBe(`${base}/attachments/5`);
    expect(publicMirror("/attachments/5/thumbnail")).toBe(`${base}/attachments/5/thumbnail`);
    expect(publicMirror("/attachments/5/preview")).toBe(`${base}/attachments/5/preview`);
  });

  test("refuses everything that has no public twin", () => {
    setPublicProject("LIF");
    for (const path of [
      "/issues",
      "/issues?project_id=3",
      "/pages",
      "/pages/7/activity",
      "/issues/9/activity",
      "/issues/9/restore",
      "/projects/3",
      "/projects/3/members",
      "/projects/3/activity",
      "/projects/3/insights",
      "/projects/3/relations",
      "/projects/3/attachments",
      "/projects/3/attachments/orphans",
      "/plans?project_id=3",
      "/plans/1",
      "/search?query=x",
      "/users",
      "/auth/keys",
      "/attachments/5/links",
      "/export/issues/LIF-1",
      "/modules",
      "/labels/4",
      "/project-archives/LIF",
      "/events/ws",
    ]) {
      expect(publicMirror(path)).toBeNull();
    }
  });

  test("answers identity, role and history locally without a request", () => {
    setPublicProject("LIF");
    expect(publicSynthetic("/auth/me")?.status).toBe(401);
    expect(publicSynthetic("/projects/3/my-role")).toEqual({
      status: 200,
      body: { role: null, enforced: true, is_admin: false },
    });
    expect(publicSynthetic("/issues/9/activity?limit=100")).toEqual({
      status: 200,
      body: { items: [], has_more: false },
    });
    expect(publicSynthetic("/pages/7/activity")?.status).toBe(200);
    expect(publicSynthetic("/projects/3/mention-candidates")).toEqual({ status: 200, body: [] });
    expect(publicSynthetic("/projects/3/views")).toEqual({ status: 200, body: [] });
    expect(publicSynthetic("/issues/9")).toBeUndefined();
  });

  test("encodes the project identifier into the path", () => {
    setPublicProject("a b");
    expect(publicMirror("/projects")).toBe("/public/api/projects/A%20B");
  });
});

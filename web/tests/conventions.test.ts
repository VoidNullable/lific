import { describe, expect, it } from "bun:test";
import { conventionsState, startingTemplates } from "../src/lib/pages/conventions";
import type { ConventionTemplate } from "../src/lib/api";

const row = (id: number, preview: string, conventions?: boolean) => ({ id, preview, conventions });

describe("conventionsState", () => {
  it("is none when no row is the conventions page", () => {
    expect(conventionsState([row(1, "notes"), row(2, "", false)])).toEqual({ kind: "none" });
    expect(conventionsState([])).toEqual({ kind: "none" });
  });

  it("is empty when the conventions page has no text", () => {
    const page = row(3, "", true);
    expect(conventionsState([row(1, "notes"), page])).toEqual({ kind: "empty", page });
  });

  it("treats a whitespace-only preview as empty", () => {
    const page = row(3, "   ", true);
    expect(conventionsState([page]).kind).toBe("empty");
  });

  it("is page when the conventions page has text", () => {
    const page = row(3, "# Conventions", true);
    expect(conventionsState([row(1, "notes"), page])).toEqual({ kind: "page", page });
  });
});

describe("startingTemplates", () => {
  const t = (key: ConventionTemplate["key"], content: string): ConventionTemplate => ({
    key,
    name: key,
    summary: "",
    content,
  });

  it("drops the empty template and keeps server order", () => {
    const keys = startingTemplates([
      t("empty", ""),
      t("ai_managed", "a"),
      t("human_managed", "b"),
      t("human_owned", "c"),
    ]).map((x) => x.key);
    expect(keys).toEqual(["ai_managed", "human_managed", "human_owned"]);
  });

  it("drops a template with no body", () => {
    expect(startingTemplates([t("ai_managed", "  ")])).toEqual([]);
  });
});

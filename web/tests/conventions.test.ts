import { describe, expect, it } from "bun:test";
import {
  FALLBACK_TEMPLATES,
  conventionsRoute,
  conventionsState,
  defaultTemplateKey,
  pageRoute,
  templateName,
} from "../src/lib/pages/conventions";
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

describe("routes", () => {
  it("opens the conventions page on its tab and any other page in Pages", () => {
    expect(conventionsRoute("LIF")).toBe("/LIF/conventions");
    expect(pageRoute("LIF", { id: 7, conventions: true })).toBe("/LIF/conventions");
    expect(pageRoute("LIF", { id: 7, conventions: false })).toBe("/LIF/pages/7");
    expect(pageRoute("LIF", { id: 7 })).toBe("/LIF/pages/7");
  });
});

describe("templates", () => {
  const t = (key: ConventionTemplate["key"], content: string): ConventionTemplate => ({
    key,
    name: key === "empty" ? "Empty" : key,
    summary: "",
    content,
  });

  it("calls the empty template Blank and keeps the other names", () => {
    expect(templateName(t("empty", ""))).toBe("Blank");
    expect(templateName(t("human_owned", "c"))).toBe("human_owned");
  });

  it("starts on the first template with text", () => {
    expect(defaultTemplateKey([t("empty", ""), t("ai_managed", "a"), t("human_owned", "c")])).toBe("ai_managed");
  });

  it("starts on the first template when none has text", () => {
    expect(defaultTemplateKey(FALLBACK_TEMPLATES)).toBe("empty");
    expect(defaultTemplateKey([])).toBe("empty");
  });
});

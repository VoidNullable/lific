import { describe, expect, test } from "bun:test";
import { marked } from "marked";
import { setTaskChecked, taskStates, toggleTask } from "../src/lib/taskList";

/** Checked states of the checkboxes marked actually renders, read off the
 *  HTML. The mapping must always agree with this. */
function renderedStates(source: string): boolean[] {
  const html = marked.parse(source.replace(/\\n/g, "\n"), { gfm: true, breaks: true }) as string;
  return [...html.matchAll(/<input ([^>]*)type="checkbox"/g)].map((m) => m[1].includes("checked"));
}

describe("taskStates", () => {
  test("matches the rendered checkboxes, skipping code and non-items", () => {
    const source = [
      "- [ ] one",
      "- [x] two",
      "",
      "```",
      "- [ ] not a task",
      "```",
      "",
      "    - [ ] indented code",
      "",
      "Inline `- [ ] code` and a [ ] bracket in prose.",
      "",
      "- [X] three",
    ].join("\n");
    expect(taskStates(source)).toEqual([false, true, true]);
    expect(taskStates(source)).toEqual(renderedStates(source));
  });

  test("an empty body has no tasks", () => {
    expect(taskStates("")).toEqual([]);
  });
});

describe("setTaskChecked", () => {
  test("ticks exactly the requested item", () => {
    const source = "- [ ] a\n- [ ] b\n- [ ] c\n";
    expect(setTaskChecked(source, 1, true)).toBe("- [ ] a\n- [x] b\n- [ ] c\n");
    expect(setTaskChecked(source, 0, true)).toBe("- [x] a\n- [ ] b\n- [ ] c\n");
    expect(setTaskChecked(source, 2, true)).toBe("- [ ] a\n- [ ] b\n- [x] c\n");
  });

  test("unticks both [x] and [X]", () => {
    expect(setTaskChecked("- [x] a\n- [X] b", 1, false)).toBe("- [x] a\n- [ ] b");
    expect(setTaskChecked("- [x] a\n- [X] b", 0, false)).toBe("- [ ] a\n- [X] b");
  });

  test("returns the source unchanged when the state already matches", () => {
    const source = "- [x] done";
    expect(setTaskChecked(source, 0, true)).toBe(source);
  });

  test("refuses an index past the last task", () => {
    expect(setTaskChecked("- [ ] only", 1, true)).toBeNull();
    expect(setTaskChecked("- [ ] only", -1, true)).toBeNull();
    expect(setTaskChecked("no tasks here", 0, true)).toBeNull();
  });

  test("skips task-like text in a fenced block that comes first", () => {
    const source = [
      "```md",
      "- [ ] not a task",
      "- [ ] nor this",
      "```",
      "~~~",
      "- [ ] tilde fence",
      "~~~",
      "- [ ] first",
      "- [ ] second",
    ].join("\n");
    const next = setTaskChecked(source, 0, true)!;
    expect(next).toBe(source.replace("- [ ] first", "- [x] first"));
    expect(setTaskChecked(source, 1, true)).toBe(source.replace("- [ ] second", "- [x] second"));
  });

  test("skips indented code and inline code", () => {
    const source = "Example:\n\n    - [ ] code\n\n`- [ ] inline`\n\n- [ ] real";
    expect(setTaskChecked(source, 0, true)).toBe(source.replace("- [ ] real", "- [x] real"));
  });

  test("a fence inside a list item hides its contents too", () => {
    const source = "- [ ] outer\n\n  ```\n  - [ ] fenced\n  ```\n- [ ] after";
    expect(taskStates(source)).toEqual([false, false]);
    expect(setTaskChecked(source, 1, true)).toBe(source.replace("- [ ] after", "- [x] after"));
  });

  test("maps nested items in render order", () => {
    const source = [
      "- [ ] parent",
      "  - [ ] child one",
      "    - [ ] grandchild",
      "  - [ ] child two",
      "- [ ] sibling",
    ].join("\n");
    expect(taskStates(source)).toEqual([false, false, false, false, false]);
    const labels = ["parent", "child one", "grandchild", "child two", "sibling"];
    labels.forEach((label, i) => {
      expect(setTaskChecked(source, i, true)).toBe(source.replace(`[ ] ${label}`, `[x] ${label}`));
    });
  });

  test("a plain parent item does not take a task index", () => {
    const source = "- plain parent\n  - [ ] child\n- [ ] sibling";
    expect(taskStates(source)).toEqual([false, false]);
    expect(setTaskChecked(source, 0, true)).toBe(source.replace("[ ] child", "[x] child"));
  });

  test("handles ordered-list task items", () => {
    const source = "1. [ ] first\n2. [x] second\n3) [ ] third";
    expect(taskStates(source)).toEqual(renderedStates(source));
    expect(setTaskChecked(source, 0, true)).toBe("1. [x] first\n2. [x] second\n3) [ ] third");
    expect(setTaskChecked(source, 1, false)).toBe("1. [ ] first\n2. [ ] second\n3) [ ] third");
  });

  test("handles tasks inside a blockquote and loose lists", () => {
    const source = "> - [ ] quoted\n\n- [ ] loose one\n\n- [ ] loose two\n";
    expect(taskStates(source)).toEqual([false, false, false]);
    expect(setTaskChecked(source, 0, true)).toBe(source.replace("[ ] quoted", "[x] quoted"));
    expect(setTaskChecked(source, 2, true)).toBe(source.replace("[ ] loose two", "[x] loose two"));
  });

  test("ignores a bracket later on the item's line", () => {
    const source = "- [ ] pick [x] or [ ]\n- [ ] next";
    expect(setTaskChecked(source, 0, true)).toBe("- [x] pick [x] or [ ]\n- [ ] next");
    expect(setTaskChecked(source, 1, true)).toBe("- [ ] pick [x] or [ ]\n- [x] next");
  });

  test("preserves CRLF line endings", () => {
    const source = "- [ ] a\r\n```\r\n- [ ] code\r\n```\r\n- [ ] b\r\n- [ ] c\r\n";
    expect(taskStates(source)).toEqual([false, false, false]);
    expect(setTaskChecked(source, 1, true)).toBe(
      "- [ ] a\r\n```\r\n- [ ] code\r\n```\r\n- [x] b\r\n- [ ] c\r\n",
    );
  });

  test("maps through legacy escaped newlines the renderer expands", () => {
    // Literal backslash-n, as stored by the LIF-10 bug.
    const source = "- [ ] a\\n- [ ] b";
    expect(taskStates(source)).toEqual([false, false]);
    expect(setTaskChecked(source, 1, true)).toBe("- [ ] a\\n- [x] b");
  });
});

describe("toggleTask", () => {
  test("flips one item and back", () => {
    const source = "- [ ] a\n- [ ] b";
    const ticked = toggleTask(source, 1)!;
    expect(ticked).toBe("- [ ] a\n- [x] b");
    expect(toggleTask(ticked, 1)).toBe(source);
  });
});

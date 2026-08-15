import { describe, expect, test } from "bun:test";
import { createMermaidBudget } from "../src/lib/mermaidLimits";
import { renderMermaidBlock } from "../src/lib/mermaidRender";

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((resolvePromise) => {
    resolve = resolvePromise;
  });
  return { promise, resolve };
}

function block(source: string): HTMLDivElement {
  return {
    dataset: { mermaid: source },
    style: {},
    textContent: "",
    innerHTML: "",
  } as unknown as HTMLDivElement;
}

describe("renderMermaidBlock", () => {
  test("rejects a forged complex placeholder before rendering", async () => {
    const target = block(encodeURIComponent(`graph TD\nA${"-->A".repeat(128)}`));
    let renders = 0;

    await renderMermaidBlock(target, async () => {
      renders += 1;
      return { svg: "<svg></svg>" };
    }, createMermaidBudget(), () => false);

    expect(renders).toBe(0);
    expect(target.textContent).toContain("too complex");
    expect(target.dataset.rendered).toBe("error");
  });

  test("rejects malformed encoded source without throwing", async () => {
    const target = block("%invalid");
    let renders = 0;

    await renderMermaidBlock(target, async () => {
      renders += 1;
      return { svg: "<svg></svg>" };
    }, createMermaidBudget(), () => false);

    expect(renders).toBe(0);
    expect(target.textContent).toContain("malformed");
  });

  test("enforces one shared budget across blocks", async () => {
    const budget = createMermaidBudget();
    const blocks = ["A", "B", "C"].map((source) => block(encodeURIComponent(source)));
    let renders = 0;

    for (const target of blocks) {
      await renderMermaidBlock(target, async () => {
        renders += 1;
        return { svg: "<svg></svg>" };
      }, budget, () => false);
    }

    expect(renders).toBe(2);
    expect(blocks[2].textContent).toContain("too many diagrams");
  });

  test("does not update after cancellation", async () => {
    const pending = deferred<{ svg: string }>();
    const target = block(encodeURIComponent("graph TD\nA-->B"));
    let cancelled = false;

    const rendering = renderMermaidBlock(
      target,
      async () => pending.promise,
      createMermaidBudget(),
      () => cancelled,
    );
    cancelled = true;
    pending.resolve({ svg: "<svg></svg>" });
    await rendering;

    expect(target.innerHTML).toBe("");
    expect(target.dataset.rendered).toBeUndefined();
  });

  test("reports success and failure while active", async () => {
    const success = block(encodeURIComponent("graph TD\nA-->B"));
    const failure = block(encodeURIComponent("graph TD\nA-->B"));

    await renderMermaidBlock(
      success,
      async () => ({ svg: "<svg>safe</svg>" }),
      createMermaidBudget(),
      () => false,
    );
    await renderMermaidBlock(
      failure,
      async () => { throw new Error("invalid diagram"); },
      createMermaidBudget(),
      () => false,
    );

    expect(success.innerHTML).toBe("<svg>safe</svg>");
    expect(success.dataset.rendered).toBe("true");
    expect(failure.textContent).toBe("Mermaid error: Error: invalid diagram");
    expect(failure.dataset.rendered).toBe("error");
  });
});

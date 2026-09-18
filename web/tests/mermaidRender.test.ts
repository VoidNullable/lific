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
    const target = block(
      encodeURIComponent(`graph TD\nA${"-->A".repeat(128)}`),
    );
    let renders = 0;

    await renderMermaidBlock(
      target,
      async () => {
        renders += 1;
        return { svg: "<svg></svg>" };
      },
      createMermaidBudget(),
      () => false,
    );

    expect(renders).toBe(0);
    expect(target.textContent).toContain("too complex");
    expect(target.dataset.rendered).toBe("error");
  });

  test("rejects malformed encoded source without throwing", async () => {
    const target = block("%invalid");
    let renders = 0;

    await renderMermaidBlock(
      target,
      async () => {
        renders += 1;
        return { svg: "<svg></svg>" };
      },
      createMermaidBudget(),
      () => false,
    );

    expect(renders).toBe(0);
    expect(target.textContent).toContain("malformed");
  });

  test("enforces one shared budget across blocks", async () => {
    const budget = createMermaidBudget();
    const blocks = ["A", "B", "C"].map((source) =>
      block(encodeURIComponent(source)),
    );
    let renders = 0;

    for (const target of blocks) {
      await renderMermaidBlock(
        target,
        async () => {
          renders += 1;
          return { svg: "<svg></svg>" };
        },
        budget,
        () => false,
      );
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

  test("a pass cancelled before it starts charges nothing and leaves the node alone", async () => {
    // The block belongs to a superseded render, so it may already be detached.
    // Even the reject paths (this source is deliberately too complex) must not
    // write to it, and nothing may be billed to a budget the next render owns.
    const budget = createMermaidBudget();
    const target = block(
      encodeURIComponent(`graph TD\nA${"-->A".repeat(128)}`),
    );
    let renders = 0;

    await renderMermaidBlock(
      target,
      async () => {
        renders += 1;
        return { svg: "<svg></svg>" };
      },
      budget,
      () => true,
    );

    expect(renders).toBe(0);
    expect(target.textContent).toBe("");
    expect(target.innerHTML).toBe("");
    expect(target.dataset.rendered).toBeUndefined();
    expect(budget).toEqual({ blocks: 0, sourceBytes: 0 });
  });

  test("a rerender gets a fresh budget, so a cancelled pass costs the next one nothing", async () => {
    // A pass cancelled *mid-render* has already claimed, and cannot un-claim:
    // the guarantee is that the abandoned budget dies with the render.
    const abandoned = createMermaidBudget();
    const pending = deferred<{ svg: string }>();
    const stale = block(encodeURIComponent("graph TD\nA-->B"));
    let cancelled = false;
    const rendering = renderMermaidBlock(
      stale,
      async () => pending.promise,
      abandoned,
      () => cancelled,
    );
    cancelled = true;
    pending.resolve({ svg: "<svg></svg>" });
    await rendering;
    expect(abandoned.blocks).toBe(1);
    expect(stale.dataset.rendered).toBeUndefined();

    // Content changed, so the next pass renders against a new budget and the
    // document is not permanently one diagram poorer.
    const fresh = createMermaidBudget();
    const blocks = ["A-->B", "C-->D"].map((edge) =>
      block(encodeURIComponent(`graph TD\n${edge}`)),
    );
    let renders = 0;
    for (const target of blocks) {
      await renderMermaidBlock(
        target,
        async () => {
          renders += 1;
          return { svg: "<svg></svg>" };
        },
        fresh,
        () => false,
      );
    }

    expect(renders).toBe(2);
    expect(blocks.map((b) => b.dataset.rendered)).toEqual(["true", "true"]);
  });

  test("a refreshed thread budget still counts the bodies already on screen", async () => {
    const render = async () => ({ svg: "<svg></svg>" });
    const bodies = (count: number) =>
      Array.from({ length: count }, (_, i) =>
        block(encodeURIComponent(`graph TD\nA${i}-->B${i}`)),
      );

    // Two comments, one diagram each, one shared budget: both draw.
    const first = bodies(2);
    const firstBudget = createMermaidBudget();
    for (const target of first) {
      await renderMermaidBlock(target, render, firstBudget, () => false);
    }
    expect(first.map((b) => b.dataset.rendered)).toEqual(["true", "true"]);

    // A third comment arrives. Every body is remounted against ONE fresh
    // budget, so the two diagrams already on screen are charged again and the
    // newcomer cannot slip past MAX_BLOCKS with an allowance of its own.
    const refreshed = bodies(3);
    const refreshedBudget = createMermaidBudget();
    for (const target of refreshed) {
      await renderMermaidBlock(target, render, refreshedBudget, () => false);
    }

    expect(refreshed.map((b) => b.dataset.rendered)).toEqual([
      "true",
      "true",
      "error",
    ]);
    expect(refreshed[2].textContent).toContain("too many diagrams");
  });

  test("a mention-roster rerender charges one shared budget, not one per body", async () => {
    const render = async () => ({ svg: "<svg></svg>" });
    const bodies = (count: number) =>
      Array.from({ length: count }, (_, i) =>
        block(encodeURIComponent(`graph TD\nA${i}-->B${i}`)),
      );

    // Three comments, one diagram each: the aggregate cap refuses the third.
    const first = bodies(3);
    const firstBudget = createMermaidBudget();
    for (const target of first) {
      await renderMermaidBlock(target, render, firstBudget, () => false);
    }
    expect(first.map((b) => b.dataset.rendered)).toEqual([
      "true",
      "true",
      "error",
    ]);

    // Candidates land and every body re-renders. One fresh budget for the whole
    // remount, so the two diagrams still on screen are charged exactly once and
    // the third is still refused — not three bodies with an allowance each, and
    // not the old budget charged twice over.
    const second = bodies(3);
    const secondBudget = createMermaidBudget();
    for (const target of second) {
      await renderMermaidBlock(target, render, secondBudget, () => false);
    }

    expect(second.map((b) => b.dataset.rendered)).toEqual([
      "true",
      "true",
      "error",
    ]);
    expect(secondBudget.blocks).toBe(2);
    expect(firstBudget).toEqual(secondBudget);
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
      async () => {
        throw new Error("invalid diagram");
      },
      createMermaidBudget(),
      () => false,
    );

    expect(success.innerHTML).toBe("<svg>safe</svg>");
    expect(success.dataset.rendered).toBe("true");
    expect(failure.textContent).toBe("Mermaid error: Error: invalid diagram");
    expect(failure.dataset.rendered).toBe("error");
  });
});

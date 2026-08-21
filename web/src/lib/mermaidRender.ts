import {
  claimMermaidBudget,
  mermaidIsTooComplex,
  type MermaidBudget,
} from "./mermaidLimits";

function showMermaidMessage(block: HTMLDivElement, message: string): void {
  block.style.color = "var(--error)";
  block.style.whiteSpace = "pre-wrap";
  block.style.margin = "0";
  block.textContent = message;
  block.dataset.rendered = "error";
}

export async function renderMermaidBlock(
  block: HTMLDivElement,
  render: (id: string, source: string) => Promise<{ svg: string }>,
  budget: MermaidBudget,
  cancelled: () => boolean,
): Promise<void> {
  let source: string;
  try {
    source = decodeURIComponent(block.dataset.mermaid ?? "");
  } catch {
    showMermaidMessage(block, "Mermaid diagram skipped: source is malformed.");
    return;
  }

  if (mermaidIsTooComplex(source)) {
    showMermaidMessage(block, "Mermaid diagram skipped: source is too complex.");
    return;
  }

  const budgetError = claimMermaidBudget(
    new TextEncoder().encode(source).byteLength,
    budget,
  );
  if (budgetError) {
    showMermaidMessage(
      block,
      budgetError === "blocks"
        ? "Mermaid diagram skipped: this document contains too many diagrams."
        : "Mermaid diagram skipped: this document contains too much diagram source.",
    );
    return;
  }

  try {
    const id = `mmd-${Math.random().toString(36).slice(2)}`;
    const { svg } = await render(id, source);
    if (!cancelled()) {
      block.innerHTML = svg;
      block.dataset.rendered = "true";
    }
  } catch (error) {
    if (!cancelled()) showMermaidMessage(block, `Mermaid error: ${String(error)}`);
  }
}

const MAX_SOURCE_BYTES = 4 * 1024;
const MAX_COMPLEXITY = 128;
const MAX_TOTAL_SOURCE_BYTES = 8 * 1024;
const MAX_BLOCKS = 2;

export type MermaidBudget = {
  blocks: number;
  sourceBytes: number;
};

export function createMermaidBudget(): MermaidBudget {
  return { blocks: 0, sourceBytes: 0 };
}

export function mermaidIsTooComplex(source: string): boolean {
  const sourceBytes = new TextEncoder().encode(source).byteLength;
  const statements = source.split(/[;\n]/).length;
  const links = source.match(/-->|==>|-\.->|---|->/g)?.length ?? 0;
  return sourceBytes > MAX_SOURCE_BYTES || statements + links > MAX_COMPLEXITY;
}

export function claimMermaidBudget(
  sourceBytes: number,
  budget: MermaidBudget,
): "blocks" | "bytes" | undefined {
  if (budget.blocks >= MAX_BLOCKS) return "blocks";
  if (budget.sourceBytes + sourceBytes > MAX_TOTAL_SOURCE_BYTES) return "bytes";
  budget.blocks += 1;
  budget.sourceBytes += sourceBytes;
}

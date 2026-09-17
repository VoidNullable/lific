const MAX_SOURCE_BYTES = 4 * 1024;
const MAX_COMPLEXITY = 128;
const MAX_TOTAL_SOURCE_BYTES = 8 * 1024;
const MAX_BLOCKS = 2;
const MAX_RADAR_TICKS = 128;

const NUMBER = "[-+]?(?:\\d+(?:\\.\\d*)?|\\.\\d+)(?:e[-+]?\\d+)?";
const XY_AXIS_RANGE = new RegExp(
  `^\\s*x-axis\\s+(${NUMBER})\\s*-->\\s*(${NUMBER})`,
  "im",
);

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
  if (sourceBytes > MAX_SOURCE_BYTES || statements + links > MAX_COMPLEXITY) {
    return true;
  }

  if (/^\s*xychart(?:-beta)?\b/im.test(source)) {
    const range = source.match(XY_AXIS_RANGE);
    if (range && Number(range[1]) === Number(range[2])) return true;
  }

  if (/^\s*radar-beta\b/im.test(source)) {
    const ticks = source.match(/^\s*ticks\s+(\d+)\s*$/im);
    if (ticks && Number(ticks[1]) > MAX_RADAR_TICKS) return true;
  }

  return (
    /^\s*architecture-beta\b/im.test(source) &&
    /^\s*group\s+(?:__proto__|prototype|constructor)\b/im.test(source)
  );
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

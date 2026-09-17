const MAX_SOURCE_BYTES = 4 * 1024;
const MAX_COMPLEXITY = 128;
const MAX_TOTAL_SOURCE_BYTES = 8 * 1024;
const MAX_BLOCKS = 2;
const MAX_RADAR_TICKS = 128;

const NUMBER = "[-+]?(?:\\d+(?:\\.\\d*)?|\\.\\d+)(?:e[-+]?\\d+)?";
const XY_AXIS_RANGE = new RegExp(
  `^x-axis\\b.*?(${NUMBER})\\s*-->\\s*(${NUMBER})\\s*$`,
  "is",
);

// Mermaid accepts semicolon-separated statements and trailing comments. Keep
// quoted labels intact so their punctuation and keywords are never directives.
function mermaidStatements(source: string): string[] {
  const statements: string[] = [];
  let statement = "";
  for (const token of source.match(/"[^"]*"|%%[^\r\n]*|[;\r\n]|[^";%\r\n]+|["%]/g) ?? []) {
    if (token.startsWith("%%")) continue;
    if (/^[;\r\n]$/.test(token)) {
      if (statement.trim()) statements.push(statement.trim());
      statement = "";
    } else {
      statement += token;
    }
  }
  if (statement.trim()) statements.push(statement.trim());
  return statements;
}

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

  const directives = mermaidStatements(source);
  if (directives.some((statement) => /^xychart(?:-beta)?\b/i.test(statement))) {
    for (const statement of directives) {
      const range = statement.match(XY_AXIS_RANGE);
      if (range && Number(range[1]) === Number(range[2])) return true;
    }
  }

  if (directives.some((statement) => /^radar-beta\b/i.test(statement))) {
    for (const statement of directives) {
      const ticks = statement.match(/^ticks\s+(\d+)\s*$/i);
      if (ticks && Number(ticks[1]) > MAX_RADAR_TICKS) return true;
    }
  }

  return (
    directives.some((statement) => /^architecture-beta\b/i.test(statement)) &&
    directives.some((statement) => /^group\s+(?:__proto__|prototype|constructor)\b/i.test(statement))
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

/** Project identifiers prefix every issue (`LIF-42`) and page (`LIF-DOC-1`).
 *  The server's grammar: 1 to 5 uppercase letters or digits, starting with a
 *  letter, and never `DOC`. These helpers keep the new-project form inside it
 *  before the server has to say no. */

export const IDENTIFIER_MAX = 5;

/** What a typed value becomes: uppercase, letters and digits only, capped. */
export function cleanIdentifier(raw: string): string {
  return raw.toUpperCase().replace(/[^A-Z0-9]/g, "").slice(0, IDENTIFIER_MAX);
}

/** The identifier suggested for a name. Leading digits are dropped so
 *  "3D Printer" gives `DPRIN`, not an id the server rejects. When the
 *  suggestion is taken (or reserved), its tail is swapped for a digit:
 *  `LIF` taken gives `LIF2`. */
export function suggestIdentifier(name: string, taken: ReadonlySet<string> = new Set()): string {
  const base = name.toUpperCase().replace(/[^A-Z0-9]/g, "").replace(/^[0-9]+/, "").slice(0, IDENTIFIER_MAX);
  if (!base) return "";
  if (!taken.has(base) && base !== "DOC") return base;
  const stem = base.slice(0, IDENTIFIER_MAX - 1);
  for (let n = 2; n <= 9; n++) {
    const candidate = `${stem}${n}`;
    if (!taken.has(candidate)) return candidate;
  }
  return base;
}

/** Why the server would refuse this identifier, in a phrase, or null.
 *  `takenBy` maps the identifiers the caller can see to project names. */
export function identifierProblem(id: string, takenBy: ReadonlyMap<string, string> = new Map()): string | null {
  if (!id) return null;
  if (!/^[A-Z]/.test(id)) return "Start with a letter";
  if (id === "DOC") return "DOC is reserved for page links";
  const owner = takenBy.get(id);
  if (owner) return `Already used by ${owner}`;
  return null;
}

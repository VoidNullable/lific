// Inbox state kept on this device: what has been read, and how the list is
// cut (tab, sort, grouping, the reading pane). Read state is per device on
// purpose: it is a personal triage aid, not something the server tracks.

import { emptyReadState, type GroupBy, type InboxTab, type ReadState, type SortBy } from "./inbox";

const READ_KEY = "lific_inbox_read";
const VIEW_KEY = "lific_inbox_view";

export interface InboxView {
  tab: InboxTab;
  sort: SortBy;
  group: GroupBy;
  pane: boolean;
}

export const DEFAULT_VIEW: InboxView = { tab: "all", sort: "priority", group: "project", pane: true };

const TABS: InboxTab[] = ["all", "unread", "assigned", "human", "waiting", "later"];
const SORTS: SortBy[] = ["priority", "newest", "oldest"];
const GROUPS: GroupBy[] = ["project", "priority", "reason", "updated", "none"];

function read(key: string): unknown {
  try {
    const raw = localStorage.getItem(key);
    return raw ? JSON.parse(raw) : null;
  } catch {
    return null;
  }
}

function write(key: string, value: unknown): void {
  try {
    localStorage.setItem(key, JSON.stringify(value));
  } catch {
    // Storage full or disabled: the inbox still works for this visit.
  }
}

export function loadReadState(): ReadState {
  const v = read(READ_KEY) as Partial<ReadState> | null;
  if (!v || typeof v !== "object" || typeof v.items !== "object" || v.items === null) return emptyReadState();
  const items: Record<number, string> = {};
  for (const [id, stamp] of Object.entries(v.items)) {
    if (typeof stamp === "string" && Number.isFinite(Number(id))) items[Number(id)] = stamp;
  }
  return { seeded: v.seeded === true, items };
}

export function saveReadState(state: ReadState): void {
  write(READ_KEY, state);
}

export function loadView(): InboxView {
  const v = read(VIEW_KEY) as Partial<InboxView> | null;
  if (!v || typeof v !== "object") return { ...DEFAULT_VIEW };
  return {
    tab: TABS.includes(v.tab as InboxTab) ? (v.tab as InboxTab) : DEFAULT_VIEW.tab,
    sort: SORTS.includes(v.sort as SortBy) ? (v.sort as SortBy) : DEFAULT_VIEW.sort,
    group: GROUPS.includes(v.group as GroupBy) ? (v.group as GroupBy) : DEFAULT_VIEW.group,
    pane: typeof v.pane === "boolean" ? v.pane : DEFAULT_VIEW.pane,
  };
}

export function saveView(view: InboxView): void {
  write(VIEW_KEY, view);
}

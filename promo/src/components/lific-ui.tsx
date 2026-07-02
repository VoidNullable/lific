import React from "react";
import { staticFile, Img } from "remotion";
import { C } from "../theme";
import { BODY, DISPLAY, MONO } from "../fonts";
import {
  ChevronRight,
  ChevronDown,
  Search,
  Plus,
  ListIcon,
  LayoutGrid,
  LayoutDashboard,
  Layers,
  FileText,
  ListChecks,
  History,
  TrendingUp,
  Home,
  Moon,
  HelpCircle,
  ArrowDown,
  SlidersVertical,
  Rows3,
  PanelLeftClose,
  Bookmark,
  SettingsGear,
  Circle,
  CircleDot,
  CircleDashed,
  CircleCheckBig,
  CircleX,
  CircleAlert,
} from "./icons";

/*
 * Pixel-faithful replica of the real web UI (web/src/lib/Layout.svelte,
 * issues/Topbar.svelte, IssueList.svelte board branch, issues/IssueCard
 * .svelte, StatusIcon.svelte, PriorityIcon.svelte). Every size below is
 * the computed CSS px of the corresponding Tailwind class in the app.
 */

// Type scale (app.css @theme)
const MICRO = 11;
const CAPTION = 12;
const BODY_SM = 13;
const HEADING = 18;

const BTN_SUCCESS = "#3bb266";
const BTN_SUCCESS_TEXT = C.stone950;

// ── StatusIcon.svelte ────────────────────────────────────────

export const statusColor = (s: string): string => {
  switch (s) {
    case "backlog":
      return C.textFaint;
    case "todo":
      return C.textMuted;
    case "active":
      return C.accent;
    case "done":
      return C.success;
    default:
      return C.textFaint;
  }
};

export const StatusIcon: React.FC<{ status: string; size?: number }> = ({
  status,
  size = 14,
}) => {
  const color = statusColor(status);
  if (status === "done") return <CircleCheckBig size={size} color={color} />;
  if (status === "cancelled") return <CircleX size={size} color={color} />;
  if (status === "active") return <CircleDot size={size} color={color} />;
  if (status === "backlog") return <CircleDashed size={size} color={color} />;
  return <Circle size={size} color={color} />;
};

// ── PriorityIcon.svelte ──────────────────────────────────────

const Bars: React.FC<{ size: number; color: string; ys: number[] }> = ({
  size,
  color,
  ys,
}) => (
  <svg
    width={size}
    height={size}
    viewBox="0 0 24 24"
    fill="none"
    stroke={color}
    strokeWidth="2"
    strokeLinecap="round"
    strokeLinejoin="round"
    style={{ flexShrink: 0, display: "block" }}
  >
    {ys.map((y) => (
      <line key={y} x1="5" y1={y} x2="19" y2={y} />
    ))}
  </svg>
);

export const PriorityIcon: React.FC<{ priority: string; size?: number }> = ({
  priority,
  size = 14,
}) => {
  if (priority === "urgent") return <CircleAlert size={size} color={C.error} />;
  if (priority === "high") return <Bars size={size} color={C.warn} ys={[6, 12, 18]} />;
  if (priority === "medium") return <Bars size={size} color={C.accent} ys={[9, 15]} />;
  if (priority === "low") return <Bars size={size} color={C.textMuted} ys={[12]} />;
  return null;
};

// ── issues/IssueCard.svelte ──────────────────────────────────

export type Label = { name: string; color: string };

export type IssueData = {
  identifier: string;
  title: string;
  priority?: "urgent" | "high" | "medium" | "low";
  labels?: Label[];
  updated: string; // formatRelative output, e.g. "2h ago"
  status?: string; // done/cancelled strike the title
};

export const IssueCard: React.FC<{
  issue: IssueData;
  width: number;
  style?: React.CSSProperties;
}> = ({ issue, width, style }) => {
  const struck = issue.status === "done" || issue.status === "cancelled";
  return (
    <article
      style={{
        width,
        backgroundColor: C.surface,
        border: `1px solid ${C.border}`,
        borderRadius: 6, // rounded-md
        padding: 10, // p-2.5
        boxSizing: "border-box",
        fontFamily: BODY,
        ...style,
      }}
    >
      {/* Top row: identifier + priority. Fixed 14px row height so cards
          with and without a priority icon keep identical rhythm. */}
      <div
        style={{
          display: "flex",
          alignItems: "center",
          height: 14,
          gap: 8, // gap-2
          marginBottom: 6, // mb-1.5
        }}
      >
        <span
          style={{
            fontSize: MICRO,
            lineHeight: "14px",
            fontFamily: MONO,
            color: C.textFaint,
          }}
        >
          {issue.identifier}
        </span>
        <div style={{ flex: 1 }} />
        {issue.priority ? (
          <PriorityIcon priority={issue.priority} size={14} />
        ) : null}
      </div>

      {/* Title. Explicit 18px line box: deterministic card heights. */}
      <h3
        style={{
          margin: 0,
          fontSize: BODY_SM,
          fontWeight: 400,
          lineHeight: "18px", // leading-snug at 13px
          color: struck ? C.textMuted : C.text,
          textDecoration: struck ? "line-through" : "none",
          whiteSpace: "nowrap",
          overflow: "hidden",
        }}
      >
        {issue.title}
      </h3>

      {/* Bottom: labels + updated time. Fixed 19px row height (chip box)
          so label-less cards match labeled ones. */}
      <div
        style={{
          display: "flex",
          alignItems: "center",
          height: 19,
          gap: 6, // gap-1.5
          marginTop: 8, // mt-2
        }}
      >
        {(issue.labels ?? []).map((lbl) => (
          <span
            key={lbl.name}
            style={{
              fontSize: MICRO,
              fontWeight: 500,
              padding: "2px 6px", // px-1.5 py-0.5
              borderRadius: 999,
              border: `1px solid ${lbl.color}40`,
              color: lbl.color,
              lineHeight: 1.3,
            }}
          >
            {lbl.name}
          </span>
        ))}
        <div style={{ flex: 1 }} />
        <span
          style={{
            fontSize: MICRO,
            color: C.textFaint,
            fontVariantNumeric: "tabular-nums",
          }}
        >
          {issue.updated}
        </span>
      </div>
    </article>
  );
};

// ── Board geometry (IssueList.svelte board branch) ───────────

export const SIDEBAR_W = 230;
export const TOPBAR_H = 44; // py-2 + h-7 row
export const PILLS_H = 46; // pt-3 pb-2 + pill row
export const COL_W = 300; // md:w-[300px]
export const COL_HEADER_H = 40; // px-3 py-2.5 + icon row + border-b
export const CARD_PAD = 8; // column p-2
export const CARD_GAP = 8; // gap-2
export const CARD_W = COL_W - CARD_PAD * 2 - 1; // minus border-r

/** x of a column's left edge, relative to the content panel (LificApp children). */
export const colX = (i: number) => i * COL_W;
/** Top y of the first card in a column, relative to the content panel. */
export const cardsTop = () => PILLS_H + COL_HEADER_H + CARD_PAD;

// ── Column header (IssueList.svelte) ─────────────────────────

export const ColumnHeader: React.FC<{ status: string; count: number }> = ({
  status,
  count,
}) => (
  <div
    style={{
      display: "flex",
      alignItems: "center",
      gap: 8, // gap-2
      padding: "10px 12px", // px-3 py-2.5
      borderBottom: `1px solid ${C.border}`,
      height: COL_HEADER_H,
      boxSizing: "border-box",
      fontFamily: BODY,
    }}
  >
    <StatusIcon status={status} size={14} />
    <span
      style={{
        fontSize: CAPTION,
        fontWeight: 600,
        textTransform: "uppercase",
        letterSpacing: "0.1em", // tracking-widest
        color: C.textMuted,
      }}
    >
      {status}
    </span>
    <span
      style={{
        fontSize: CAPTION,
        color: C.textFaint,
        fontVariantNumeric: "tabular-nums",
      }}
    >
      {count}
    </span>
    <div style={{ flex: 1 }} />
    <PanelLeftClose size={12} color={C.textFaint} />
    <Plus size={12} color={C.textFaint} />
  </div>
);

// ── Columns visibility pill bar (IssueList.svelte) ───────────

const ColumnsPillBar: React.FC<{
  statuses: { status: string; count: number; visible: boolean }[];
}> = ({ statuses }) => (
  <div
    style={{
      display: "flex",
      alignItems: "center",
      gap: 12, // gap-3
      padding: "12px 24px 8px", // px-6 pt-3 pb-2
      fontFamily: BODY,
    }}
  >
    <span
      style={{
        fontSize: MICRO,
        fontWeight: 600,
        textTransform: "uppercase",
        letterSpacing: "0.1em",
        color: C.textFaint,
      }}
    >
      Columns
    </span>
    <div
      style={{
        display: "flex",
        alignItems: "center",
        gap: 2, // gap-0.5
        padding: 2, // p-0.5
        borderRadius: 6,
        backgroundColor: C.bgSubtle,
        border: `1px solid ${C.border}`,
      }}
    >
      {statuses.map(({ status, count, visible }) => (
        <div
          key={status}
          style={{
            display: "flex",
            alignItems: "center",
            gap: 6, // gap-1.5
            padding: "4px 8px", // px-2 py-1
            borderRadius: 4,
            fontSize: CAPTION,
            fontWeight: 500,
            color: visible ? C.text : C.textFaint,
            backgroundColor: visible ? C.chrome : "transparent",
            boxShadow: visible ? "0 1px 2px rgba(0,0,0,0.08)" : "none",
          }}
        >
          <StatusIcon status={status} size={12} />
          <span style={{ textTransform: "capitalize" }}>{status}</span>
          <span
            style={{
              fontSize: MICRO,
              color: visible ? C.textMuted : C.textFaint,
              fontVariantNumeric: "tabular-nums",
            }}
          >
            {count}
          </span>
        </div>
      ))}
    </div>
  </div>
);

// ── issues/Topbar.svelte (board flavor) ──────────────────────

const Sep: React.FC = () => (
  <div style={{ width: 1, height: 16, backgroundColor: C.border }} />
);

const TopbarBtn: React.FC<{
  icon: React.ReactNode;
  label?: string;
}> = ({ icon, label }) => (
  <div
    style={{
      height: 28, // h-7
      display: "flex",
      alignItems: "center",
      gap: 4, // gap-1
      padding: "0 8px", // px-2
      borderRadius: 6,
      fontSize: CAPTION,
      fontWeight: 500,
      color: C.textMuted,
    }}
  >
    {icon}
    {label ? <span>{label}</span> : null}
  </div>
);

const Topbar: React.FC<{
  tallies: { status: string; count: number }[];
  countLabel: string;
}> = ({ tallies, countLabel }) => (
  <div
    style={{
      height: TOPBAR_H,
      display: "flex",
      alignItems: "center",
      gap: 12, // gap-3
      padding: "8px 24px", // px-6 py-2
      boxSizing: "border-box",
      backgroundColor: C.chrome,
      fontFamily: BODY,
    }}
  >
    {/* Breadcrumb */}
    <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
      <span
        style={{
          fontSize: BODY_SM,
          fontFamily: MONO,
          fontWeight: 500,
          color: C.textMuted,
        }}
      >
        LIF
      </span>
      <ChevronRight size={12} color={C.textFaint} />
      <span style={{ fontSize: BODY_SM, fontWeight: 500, color: C.text }}>
        Board
      </span>
    </div>

    {/* View switcher pill */}
    <div
      style={{
        display: "flex",
        alignItems: "center",
        gap: 2,
        padding: 2,
        borderRadius: 6,
        backgroundColor: C.bg,
        boxShadow: "inset 0 1px 2px rgba(0,0,0,0.10)",
      }}
    >
      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: 4,
          padding: "2px 8px",
          borderRadius: 4,
          fontSize: CAPTION,
          fontWeight: 500,
          color: C.textMuted,
        }}
      >
        <ListIcon size={11} color={C.textMuted} />
        List
      </div>
      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: 4,
          padding: "2px 8px",
          borderRadius: 4,
          fontSize: CAPTION,
          fontWeight: 500,
          color: C.text,
          backgroundColor: C.surface,
          boxShadow:
            "0 1px 2px rgba(0,0,0,0.16), 0 1px 1px rgba(0,0,0,0.10)",
        }}
      >
        <LayoutGrid size={11} color={C.text} />
        Board
      </div>
    </div>

    {/* Status tallies */}
    <div style={{ display: "flex", alignItems: "center", gap: 2 }}>
      {tallies.map(({ status, count }) => (
        <div
          key={status}
          style={{
            height: 24, // h-6
            display: "flex",
            alignItems: "center",
            gap: 4,
            padding: "0 6px", // px-1.5
            borderRadius: 4,
            fontSize: MICRO,
            fontWeight: 500,
            color: C.textMuted,
            fontVariantNumeric: "tabular-nums",
          }}
        >
          <StatusIcon status={status} size={12} />
          {count}
        </div>
      ))}
    </div>

    <Sep />
    <TopbarBtn icon={<SlidersVertical size={12} color={C.textMuted} />} label="Filter" />

    <div style={{ marginLeft: "auto", display: "flex", alignItems: "center", gap: 2 }}>
      <span
        style={{
          marginRight: 6,
          fontSize: MICRO,
          fontWeight: 500,
          color: C.textFaint,
          fontVariantNumeric: "tabular-nums",
        }}
      >
        {countLabel}
      </span>
      <Sep />
      <TopbarBtn icon={<Bookmark size={12} color={C.textMuted} />} label="Views" />
      <TopbarBtn icon={<ArrowDown size={12} color={C.textMuted} />} label="Priority" />
      <TopbarBtn icon={<Rows3 size={12} color={C.textMuted} />} label="Lanes" />
      <div
        style={{
          width: 28,
          height: 28,
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
        }}
      >
        <Search size={14} color={C.textMuted} />
      </div>
      <div
        style={{
          width: 28,
          height: 28,
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
        }}
      >
        <HelpCircle size={14} color={C.textMuted} />
      </div>
      <div style={{ margin: "0 6px" }}>
        <Sep />
      </div>
      {/* New issue split button */}
      <div
        style={{
          display: "flex",
          alignItems: "stretch",
          height: 28,
          borderRadius: 6,
          overflow: "hidden",
          boxShadow: "0 1px 2px rgba(0,0,0,0.05)",
        }}
      >
        <div
          style={{
            display: "flex",
            alignItems: "center",
            gap: 6,
            padding: "0 8px 0 10px",
            fontSize: BODY_SM,
            fontWeight: 500,
            color: BTN_SUCCESS_TEXT,
            backgroundColor: BTN_SUCCESS,
          }}
        >
          <Plus size={14} color={BTN_SUCCESS_TEXT} />
          New
          <span
            style={{
              display: "grid",
              placeItems: "center",
              minWidth: 17,
              height: 17,
              marginLeft: 2,
              borderRadius: 4,
              backgroundColor: "rgba(255,255,255,0.20)",
              fontFamily: MONO,
              fontSize: MICRO,
              lineHeight: 1,
            }}
          >
            C
          </span>
        </div>
        <div style={{ width: 1, backgroundColor: "rgba(255,255,255,0.25)" }} />
        <div
          style={{
            display: "flex",
            alignItems: "center",
            padding: "0 6px",
            color: BTN_SUCCESS_TEXT,
            backgroundColor: BTN_SUCCESS,
          }}
        >
          <ChevronDown size={14} color={BTN_SUCCESS_TEXT} />
        </div>
      </div>
    </div>
  </div>
);

// ── Layout.svelte sidebar ────────────────────────────────────

const NavItem: React.FC<{
  icon: React.ReactNode;
  label: string;
  active?: boolean;
  indentIconGap?: number;
}> = ({ icon, label, active }) => (
  <div
    style={{
      display: "flex",
      alignItems: "center",
      gap: 8, // gap-2
      padding: "4px 8px", // px-2 py-1 (sub-nav)
      borderRadius: 6,
      fontSize: BODY_SM,
      fontWeight: active ? 500 : 400,
      color: active ? C.text : C.textMuted,
      backgroundColor: active ? C.bgSubtle : "transparent",
    }}
  >
    {icon}
    {label}
  </div>
);

const Sidebar: React.FC = () => {
  const sub = (
    Icon: React.FC<{ size: number; color: string }>,
    label: string,
    active = false,
  ) => (
    <NavItem
      key={label}
      icon={<Icon size={14} color={active ? C.accent : C.textMuted} />}
      label={label}
      active={active}
    />
  );

  return (
    <div
      style={{
        width: SIDEBAR_W,
        flexShrink: 0,
        display: "flex",
        flexDirection: "column",
        backgroundColor: C.chrome,
        fontFamily: BODY,
        boxSizing: "border-box",
      }}
    >
      {/* Brand header */}
      <div
        style={{
          padding: "12px 12px 8px", // px-3 pt-3 pb-2
          display: "flex",
          alignItems: "center",
          gap: 10,
        }}
      >
        <div
          style={{
            display: "flex",
            flex: 1,
            alignItems: "center",
            gap: 10, // gap-2.5
            padding: "4px 4px",
          }}
        >
          <Img
            src={staticFile("logo.webp")}
            style={{ width: 26, height: 26, borderRadius: 6 }}
          />
          <span
            style={{
              fontFamily: DISPLAY,
              fontSize: HEADING,
              letterSpacing: "-0.02em",
              color: C.text,
              lineHeight: 1,
              flex: 1,
              fontWeight: 600,
            }}
          >
            Lific
          </span>
          <span
            style={{
              fontFamily: MONO,
              fontSize: MICRO,
              letterSpacing: "-0.02em",
              color: C.textFaint,
              padding: "2px 6px",
              borderRadius: 6,
              backgroundColor: C.bgSubtle,
            }}
          >
            v2.0.0
          </span>
        </div>
      </div>

      {/* Jump to… */}
      <div style={{ padding: "0 12px 8px" }}>
        <div
          style={{
            height: 32, // h-8
            display: "flex",
            alignItems: "center",
            gap: 8,
            padding: "0 10px",
            borderRadius: 6,
            backgroundColor: C.bg,
            boxShadow: "inset 0 1px 2px rgba(0,0,0,0.08)",
            color: C.textMuted,
          }}
        >
          <Search size={14} color={C.textMuted} />
          <span style={{ flex: 1, fontSize: BODY_SM }}>Jump to…</span>
          <span
            style={{
              fontFamily: MONO,
              fontSize: MICRO,
              lineHeight: 1,
              color: C.textFaint,
              border: `1px solid ${C.border}`,
              borderRadius: 4,
              padding: "2px 4px",
            }}
          >
            ⌘K
          </span>
        </div>
      </div>

      {/* Nav */}
      <div style={{ flex: 1, padding: "4px 8px" }}>
        <div style={{ marginBottom: 4, padding: "6px 10px", display: "flex", alignItems: "center", gap: 8, borderRadius: 6, fontSize: BODY_SM, color: C.textMuted }}>
          <Home size={14} color={C.textMuted} />
          Home
        </div>

        <div
          style={{
            display: "flex",
            alignItems: "center",
            justifyContent: "space-between",
            padding: "6px 8px 4px",
          }}
        >
          <span
            style={{
              fontSize: MICRO,
              fontWeight: 600,
              textTransform: "uppercase",
              letterSpacing: "0.1em",
              color: C.textFaint,
            }}
          >
            Projects
          </span>
          <Plus size={13} color={C.textFaint} />
        </div>

        {/* Active project pill */}
        <div
          style={{
            display: "flex",
            alignItems: "center",
            gap: 6, // gap-1.5
            padding: "6px 8px 6px 6px", // pl-1.5 pr-2 py-1.5
            borderRadius: 6,
            fontSize: BODY_SM,
            fontWeight: 500,
            color: C.text,
            backgroundColor: C.bgSubtle,
          }}
        >
          <ChevronRight size={13} color={C.textMuted} rotated />
          <span
            style={{
              width: 20,
              height: 20,
              borderRadius: 6,
              border: `1px solid ${C.border}`,
              backgroundColor: C.bgSubtle,
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              fontSize: MICRO,
              fontWeight: 600,
              letterSpacing: "-0.02em",
              color: C.text,
            }}
          >
            LI
          </span>
          <span style={{ flex: 1 }}>Lific</span>
        </div>

        {/* Sub-nav with tree guide line */}
        <div
          style={{
            marginLeft: 18, // ml-[1.125rem]
            paddingLeft: 10, // pl-2.5
            marginTop: 2,
            marginBottom: 6,
            borderLeft: `1px solid ${C.border}`,
            display: "flex",
            flexDirection: "column",
            gap: 1,
          }}
        >
          {sub(LayoutDashboard, "Overview")}
          {sub(ListIcon, "Issues")}
          {sub(LayoutGrid, "Board", true)}
          {sub(Layers, "Modules")}
          {sub(FileText, "Pages")}
          {sub(ListChecks, "Plans")}
          {sub(History, "Activity")}
          {sub(TrendingUp, "Insights")}
        </div>
      </div>

      {/* Footer */}
      <div
        style={{
          padding: 8,
          display: "flex",
          alignItems: "center",
          gap: 4,
        }}
      >
        <div
          style={{
            flex: 1,
            display: "flex",
            alignItems: "center",
            gap: 10,
            padding: "6px 8px",
            borderRadius: 6,
          }}
        >
          <div
            style={{
              width: 28,
              height: 28,
              borderRadius: 14,
              backgroundColor: C.accent,
              color: C.stone950,
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              fontSize: MICRO,
              fontWeight: 600,
              letterSpacing: "0.02em",
            }}
          >
            L
          </div>
          <div style={{ flex: 1, minWidth: 0 }}>
            <div style={{ fontSize: BODY_SM, color: C.text, lineHeight: 1.25 }}>
              Lizzy
            </div>
            <div
              style={{
                fontSize: MICRO,
                color: C.textFaint,
                display: "flex",
                alignItems: "center",
                gap: 4,
                marginTop: 2,
                lineHeight: 1.25,
              }}
            >
              <SettingsGear size={9} color={C.textFaint} /> Settings
            </div>
          </div>
        </div>
        <div style={{ width: 32, height: 32, display: "grid", placeItems: "center" }}>
          <Moon size={15} color={C.textMuted} />
        </div>
        <div style={{ width: 32, height: 32, display: "grid", placeItems: "center" }}>
          <HelpCircle size={15} color={C.textMuted} />
        </div>
      </div>
    </div>
  );
};

// ── Whole-app shell: L-chrome + recessed content panel ───────

export const BOARD_STATUSES = ["todo", "active", "done"];
const ALL_STATUSES = ["backlog", "todo", "active", "done", "cancelled"];

/**
 * The full app frame at native CSS px. Children render into the board
 * area (absolutely positioned over the column tracks).
 *
 * `columns` picks which statuses render as tracks (the rest appear as
 * hidden pills, like the real Columns visibility control). `counts`
 * drives the topbar tallies + pill bar + column headers so a drag can
 * update every count the way the live app would.
 */
export const LificApp: React.FC<{
  width: number;
  height: number;
  counts: Record<string, number>;
  totalLabel: string;
  columns?: string[];
  children?: React.ReactNode;
}> = ({ width, height, counts, totalLabel, columns = BOARD_STATUSES, children }) => {
  return (
    <div
      style={{
        width,
        height,
        display: "flex",
        backgroundColor: C.chrome,
        overflow: "hidden",
        position: "relative",
        fontFamily: BODY,
      }}
    >
      <Sidebar />
      <div
        style={{
          flex: 1,
          minWidth: 0,
          display: "flex",
          flexDirection: "column",
        }}
      >
        <Topbar
          tallies={ALL_STATUSES.filter((s) => (counts[s] ?? 0) > 0).map((s) => ({
            status: s,
            count: counts[s] ?? 0,
          }))}
          countLabel={totalLabel}
        />
        {/* Recessed content panel: rounded-tl-xl + cast shadows */}
        <div
          style={{
            position: "relative",
            flex: 1,
            minWidth: 0,
            overflow: "hidden",
            borderTopLeftRadius: 12,
            backgroundColor: C.bg,
          }}
        >
          <ColumnsPillBar
            statuses={ALL_STATUSES.map((s) => ({
              status: s,
              count: counts[s] ?? 0,
              visible: columns.includes(s),
            }))}
          />
          {/* Column tracks */}
          <div
            style={{
              position: "absolute",
              top: PILLS_H,
              left: 0,
              right: 0,
              bottom: 0,
              display: "flex",
            }}
          >
            {columns.map((s) => (
              <div
                key={s}
                style={{
                  width: COL_W,
                  flexShrink: 0,
                  borderRight: `1px solid ${C.border}`,
                  boxSizing: "border-box",
                }}
              >
                <ColumnHeader status={s} count={counts[s] ?? 0} />
              </div>
            ))}
          </div>
          {/* Cast shadows (top + left edges of the recessed panel) */}
          <div
            style={{
              pointerEvents: "none",
              position: "absolute",
              top: 0,
              left: 0,
              right: 0,
              height: 24,
              background: "linear-gradient(to bottom, rgba(0,0,0,0.17), transparent)",
            }}
          />
          <div
            style={{
              pointerEvents: "none",
              position: "absolute",
              top: 0,
              left: 0,
              bottom: 0,
              width: 24,
              background: "linear-gradient(to right, rgba(0,0,0,0.17), transparent)",
            }}
          />
          {/* Scene content (cards, cursor, drop outlines) */}
          {children}
        </div>
      </div>
    </div>
  );
};

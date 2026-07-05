import React from "react";
import { C } from "../theme";
import { BODY, MONO } from "../fonts";
import {
  Sidebar,
  Topbar,
  StatusIcon,
  PriorityIcon,
  IssueData,
  Label,
} from "./lific-ui";
import { ChevronRight } from "./icons";

/*
 * Pixel-faithful replica of the web UI's ISSUES LIST view — the list
 * branch of web/src/routes/IssueList.svelte plus web/src/lib/issues/
 * IssueRow.svelte. Every size below is the computed CSS px of the
 * corresponding Tailwind class in the app (sm+ breakpoint, compact
 * density — the default desktop view).
 *
 * Group headers: IssueList.svelte:1958-1983
 *   px-6 py-2, bg-[--surface], border-b; chevron (rotated when expanded)
 *   + StatusIcon(14) + caption(12) uppercase tracking-widest text-muted
 *   + caption(12) count text-faint.
 * Rows: IssueRow.svelte:143-475
 *   gap-3 px-6 py-2.5 (compact), border-b, border-l-2; size-4 checkbox
 *   slot, size-4 StatusIcon(16), body-sm(13) mono w-[72px] identifier,
 *   flex-1 body(14) title (line-through text-muted when done/cancelled),
 *   micro(11) label chips px-1.5 py-0.5 rounded-full border ${color}40,
 *   w-9 priority (PriorityIcon 21, right), w-[60px] caption(12) time.
 */

// Type scale (app.css @theme)
const MICRO = 11;
const CAPTION = 12;
const BODY_SM = 13;
const BODY_TXT = 14;

// Row geometry (IssueRow.svelte compact density, sm+ breakpoint).
export const ROW_GAP = 12; // gap-3
export const ROW_PX = 24; // px-6
export const ROW_PY = 10; // py-2.5 (compact)
export const GROUP_HEADER_H = 33; // px-6 py-2 (8+8) + ~16 line + border adjust

/** A single list group: a status bucket with its rows. */
export type ListGroup = {
  /** status key — drives the header icon + label + group order. */
  status: string;
  issues: IssueData[];
};

// ── Group header (IssueList.svelte grouped view) ─────────────

export const GroupHeader: React.FC<{ status: string; count: number }> = ({
  status,
  count,
}) => (
  <div
    style={{
      display: "flex",
      alignItems: "center",
      gap: 8, // gap-2
      padding: "8px 24px", // px-6 py-2
      backgroundColor: C.surface,
      borderBottom: `1px solid ${C.border}`,
      boxSizing: "border-box",
      fontFamily: BODY,
    }}
  >
    {/* Expanded groups render the chevron rotated 90deg. */}
    <ChevronRight size={13} color={C.textFaint} rotated />
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
  </div>
);

// ── issues/IssueRow.svelte ───────────────────────────────────

export const IssueRow: React.FC<{
  issue: IssueData;
  isLast?: boolean;
  focused?: boolean;
  /** 0..1 cursor-hover highlight (bg-[var(--bg-subtle)], like the real
   *  row's hover state). Independent of `focused` (keyboard selection). */
  hover?: number;
  style?: React.CSSProperties;
}> = ({ issue, isLast = false, focused = false, hover = 0, style }) => {
  const struck = issue.status === "done" || issue.status === "cancelled";
  // Hover tints the row toward --bg-subtle; focus (accent) wins if set.
  const bg = focused
    ? C.accentSubtle
    : hover > 0
      ? `rgba(23,28,26,${hover})` // C.bgSubtle (#171c1a) at `hover`
      : "transparent";
  return (
    <div
      style={{
        display: "flex",
        alignItems: "center",
        gap: ROW_GAP,
        padding: `${ROW_PY}px ${ROW_PX}px`,
        borderBottom: isLast ? "none" : `1px solid ${C.border}`,
        // border-l-2 — accent when focused, else transparent
        borderLeft: `2px solid ${focused ? C.accent : "transparent"}`,
        backgroundColor: bg,
        boxSizing: "border-box",
        fontFamily: BODY,
        ...style,
      }}
    >
      {/* Selection checkbox slot: size-4, invisible until hover/selection.
          Reserved so rows never shift (IssueRow.svelte:182-204). */}
      <div style={{ width: 16, height: 16, flexShrink: 0 }} />

      {/* Status indicator — size-4 button box, StatusIcon size 16. */}
      <div
        style={{
          width: 16,
          height: 16,
          flexShrink: 0,
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
        }}
      >
        <StatusIcon status={issue.status ?? "backlog"} size={16} />
      </div>

      {/* Identifier — body-sm mono text-faint, w-[72px] (sm), truncate. */}
      <span
        style={{
          width: 72,
          flexShrink: 0,
          fontSize: BODY_SM,
          fontFamily: MONO,
          color: C.textFaint,
          whiteSpace: "nowrap",
          overflow: "hidden",
          textOverflow: "ellipsis",
        }}
      >
        {issue.identifier}
      </span>

      {/* Title column — flex-1, body(14). Done/cancelled strike + muted. */}
      <div
        style={{
          flex: 1,
          minWidth: 0,
          display: "flex",
          flexDirection: "column",
          gap: 2, // gap-0.5
        }}
      >
        <span
          style={{
            fontSize: BODY_TXT,
            color: struck ? C.textMuted : C.text,
            textDecoration: struck ? "line-through" : "none",
            whiteSpace: "nowrap",
            overflow: "hidden",
            textOverflow: "ellipsis",
          }}
        >
          {issue.title}
        </span>
      </div>

      {/* Labels — micro chips, up to 2 shown, +N overflow. */}
      {issue.labels && issue.labels.length > 0 ? (
        <div
          style={{
            display: "flex",
            alignItems: "center",
            gap: 4, // gap-1
            flexShrink: 0,
          }}
        >
          {issue.labels.slice(0, 2).map((lbl: Label) => (
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
                whiteSpace: "nowrap",
              }}
            >
              {lbl.name}
            </span>
          ))}
          {issue.labels.length > 2 ? (
            <span style={{ fontSize: MICRO, color: C.textFaint }}>
              +{issue.labels.length - 2}
            </span>
          ) : null}
        </div>
      ) : null}

      {/* Priority — w-9 box, right-aligned, PriorityIcon size 21. 'none'
          renders nothing here (its hover-only affordance is omitted). */}
      <div
        style={{
          width: 36,
          flexShrink: 0,
          display: "flex",
          alignItems: "center",
          justifyContent: "flex-end",
        }}
      >
        {issue.priority ? (
          <PriorityIcon priority={issue.priority} size={21} />
        ) : null}
      </div>

      {/* Updated time — caption text-faint, w-[60px], right-aligned. */}
      <span
        style={{
          width: 60,
          flexShrink: 0,
          textAlign: "right",
          fontSize: CAPTION,
          color: C.textFaint,
          fontVariantNumeric: "tabular-nums",
        }}
      >
        {issue.updated}
      </span>
    </div>
  );
};

// ── Whole-app frame: L-chrome + recessed content = ISSUES LIST ───

/**
 * The full app frame at native CSS px, with the content panel showing the
 * grouped ISSUES LIST (not the board). Reuses Sidebar + Topbar (List
 * active) from lific-ui. `rowReveal(globalIndex)` lets the scene stagger
 * rows in (returns {opacity, dy}); omit for a static frame.
 */
export const IssueListPage: React.FC<{
  width: number;
  height: number;
  groups: ListGroup[];
  counts: Record<string, number>;
  totalLabel: string;
  rowReveal?: (globalIndex: number) => { opacity: number; dy: number };
  /** 0..1 crossfade of the List|Board switcher pill (0 = List active). */
  switchT?: number;
  /** Flat row index the cursor is hovering (subtle bg highlight). */
  hoverIndex?: number;
  /** Strength of the hover highlight, 0..1. */
  hoverStrength?: number;
}> = ({
  width,
  height,
  groups,
  counts,
  totalLabel,
  rowReveal,
  switchT,
  hoverIndex,
  hoverStrength = 0,
}) => {
  // Flat running index so the scene can stagger rows across group borders.
  let rowIndex = -1;

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
      <Sidebar active="issues" />
      <div
        style={{
          flex: 1,
          minWidth: 0,
          display: "flex",
          flexDirection: "column",
        }}
      >
        <Topbar
          active="list"
          counts={counts}
          countLabel={totalLabel}
          switchT={switchT}
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
          {/* Scrollable list body (flex-1 overflow-y-auto). */}
          <div style={{ position: "absolute", inset: 0, overflow: "hidden" }}>
            {groups.map((g, gi) => (
              <div
                key={g.status}
                style={{
                  borderBottom:
                    gi === groups.length - 1
                      ? "none"
                      : `1px solid ${C.border}`,
                }}
              >
                <GroupHeader status={g.status} count={g.issues.length} />
                {g.issues.map((issue, si) => {
                  rowIndex += 1;
                  const rev = rowReveal
                    ? rowReveal(rowIndex)
                    : { opacity: 1, dy: 0 };
                  return (
                    <div
                      key={issue.identifier}
                      style={{
                        opacity: rev.opacity,
                        transform: `translateY(${rev.dy}px)`,
                      }}
                    >
                      <IssueRow
                        issue={issue}
                        isLast={si === g.issues.length - 1}
                        hover={hoverIndex === rowIndex ? hoverStrength : 0}
                      />
                    </div>
                  );
                })}
              </div>
            ))}
          </div>

          {/* Cast shadows (top + left edges of the recessed panel). */}
          <div
            style={{
              pointerEvents: "none",
              position: "absolute",
              top: 0,
              left: 0,
              right: 0,
              height: 24,
              background:
                "linear-gradient(to bottom, rgba(0,0,0,0.17), transparent)",
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
              background:
                "linear-gradient(to right, rgba(0,0,0,0.17), transparent)",
            }}
          />
        </div>
      </div>
    </div>
  );
};

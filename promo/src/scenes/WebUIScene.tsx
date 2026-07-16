import React from "react";
import {
  AbsoluteFill,
  useCurrentFrame,
  useVideoConfig,
  spring,
  interpolate,
  Easing,
} from "remotion";
import { C } from "../theme";
import { BODY } from "../fonts";
import { Background } from "../components/Background";
import { BrowserFrame } from "../components/BrowserFrame";
import {
  LificApp,
  IssueCard,
  IssueData,
  Label,
  colX,
  cardsTop,
  CARD_W,
  CARD_PAD,
  COL_W,
  PILLS_H,
  COL_HEADER_H,
  TOPBAR_H,
  SIDEBAR_W,
} from "../components/lific-ui";
import {
  IssueListPage,
  ListGroup,
  GROUP_HEADER_H,
} from "../components/issue-list-ui";
import { IssueDetailPage, IssueDetailData } from "../components/issue-detail-ui";
import { Cursor, Waypoint } from "../components/Cursor";

/*
 * WebUIScene — a real web UI beat for Ad B. Opens on the pixel-faithful
 * ISSUES LIST, the cursor drifts to the topbar view switcher, CLICKS
 * "Board" (the List|Board pill flips), the content crossfades list -> board,
 * then a FAST drag-and-drop reflows a card with live count updates.
 *
 * Same issue world as UIScene (LIF-231/214/207/198/226/183/171) so the list
 * and the board show the same seven issues. All motion is a pure function of
 * useCurrentFrame() (spring/interpolate with clamp); no CSS animation, no
 * Math.random. Colors from theme C, fonts from ../fonts only.
 */

// ── Shared issue world (identical to UIScene) ────────────────
const L: Record<string, Label> = {
  webui: { name: "web-ui", color: "#4dd9c7" },
  core: { name: "core", color: "#9287d7" },
  mcp: { name: "mcp", color: "#b48af0" },
  auth: { name: "auth", color: "#fb923c" },
  bug: { name: "bug", color: "#f87171" },
};

type BoardCard = { issue: IssueData; col: number; slot: number };

// col: 0 todo, 1 active, 2 done — the board layout from UIScene.
const CARDS: BoardCard[] = [
  { col: 0, slot: 0, issue: { identifier: "LIF-231", title: "Board column virtualization", priority: "medium", labels: [L.webui], updated: "2d ago", status: "todo" } },
  { col: 0, slot: 1, issue: { identifier: "LIF-214", title: "Bulk-edit issues from the list", priority: "high", labels: [L.webui], updated: "4h ago", status: "todo" } },
  { col: 0, slot: 2, issue: { identifier: "LIF-207", title: "Saved filters per project", priority: "low", updated: "1d ago", status: "todo" } },
  { col: 1, slot: 0, issue: { identifier: "LIF-198", title: "Fix WAL checkpoint race", priority: "high", labels: [L.core, L.bug], updated: "26m ago", status: "active" } },
  { col: 1, slot: 1, issue: { identifier: "LIF-226", title: "MCP: recurring plan templates", priority: "medium", labels: [L.mcp], updated: "2h ago", status: "active" } },
  { col: 2, slot: 0, issue: { identifier: "LIF-183", title: "OAuth device flow for CLI", labels: [L.auth], updated: "5h ago", status: "done" } },
  { col: 2, slot: 1, issue: { identifier: "LIF-171", title: "Backup retention config", labels: [L.core], updated: "1d ago", status: "done" } },
];

// The ISSUES LIST groups: the same seven issues bucketed by status, in
// canonical group order (grouping.ts STATUSES: ...todo, active, done...).
const LIST_GROUPS: ListGroup[] = [
  { status: "todo", issues: CARDS.filter((c) => c.col === 0).map((c) => c.issue) },
  { status: "active", issues: CARDS.filter((c) => c.col === 1).map((c) => c.issue) },
  { status: "done", issues: CARDS.filter((c) => c.col === 2).map((c) => c.issue) },
];
const TOTAL_ROWS = LIST_GROUPS.reduce((n, g) => n + g.issues.length, 0);

// Inline <code> styling (app.css .prose code), shared by the description.
const INLINE_CODE: React.CSSProperties = {
  fontFamily: "'Fira Code', ui-monospace, monospace",
  fontSize: "0.8125em",
  backgroundColor: "#171c1a",
  border: "1px solid #3d4842",
  borderRadius: 4,
  padding: "0.15em 0.4em",
  color: "#e3e8e6",
};

// ── Issue DETAIL content for LIF-198 (the detail-beat target) ──
// The same issue as the board's active card, expanded into the full
// two-column detail view (title / description / activity / comments +
// metadata aside). Content is calm and readable at 1.42x.
const DETAIL: IssueDetailData = {
  identifier: "LIF-198",
  title: "Fix WAL checkpoint race",
  status: "active",
  priority: "high",
  labels: [L.core, L.bug],
  updated: "26m ago",
  module: "Storage",
  created: "Jul 3, 2026, 9:41 AM",
  updatedAbs: "Jul 5, 2026, 2:58 PM",
  description: (
    <>
      <p style={{ margin: "0 0 0.75em" }}>
        Under sustained write load the 30-minute backup job can fire while a
        WAL checkpoint is mid-flight, so the copied <code style={INLINE_CODE}>
          .db
        </code>{" "}
        trails its <code style={INLINE_CODE}>-wal</code> by a few frames.
      </p>
      <p style={{ margin: "0 0 0.5em" }}>Repro: hammer writes, then</p>
      <ul style={{ margin: "0 0 0.75em", paddingLeft: "1.5em" }}>
        <li style={{ marginBottom: "0.25em" }}>
          Take <code style={INLINE_CODE}>PRAGMA wal_checkpoint(TRUNCATE)</code>{" "}
          before snapshotting.
        </li>
        <li style={{ marginBottom: "0.25em" }}>
          Hold the checkpoint lock for the copy, not just the open.
        </li>
        <li>Add a restore assertion that the WAL is empty.</li>
      </ul>
    </>
  ),
  activity: [
    {
      actor: "opencode",
      bot: true,
      text: (
        <>
          changed status{" "}
          <span style={{ color: C.textFaint }}>todo</span>{" "}
          <span style={{ color: C.textFaint }}>&rarr;</span>{" "}
          <span style={{ color: C.text }}>active</span>
        </>
      ),
      time: "26m ago",
    },
    {
      actor: "Lizzy",
      text: <>set priority high</>,
      time: "1h ago",
    },
    {
      actor: "Lizzy",
      text: <>created this issue</>,
      time: "2d ago",
    },
  ],
  comments: [
    {
      author: "Lizzy",
      initials: "LI",
      time: "1h ago",
      body: "Confirmed on the prod box, only shows up above ~200 writes/s. Backups otherwise fine.",
    },
    {
      author: "opencode",
      initials: "OC",
      bot: true,
      time: "26m ago",
      body: "Reproduced and moved to active. Checkpoint-then-copy under the lock looks like the clean fix.",
    },
  ],
};

// Deterministic card metrics (single-line titles): 87px card, 8px gap.
const CARD_H = 87;
const PITCH = CARD_H + 8;
const slotYAt = (slot: number) => cardsTop() + slot * PITCH;

// The moved card: LIF-214 (todo slot 1) -> active slot 2.
const MOVED = "LIF-214";

// ── Native app size (3 board columns), zoomed for phone legibility ──
const APP_W = 1146;
const APP_H = 620;
const SCALE = 1.42;

// ── Beat table (scene-local, 30fps; beat = 13.846f @ 130 BPM) ──
// Detail round-trip (fills the old dead air before the Board click).
const ROW_CLICK = 41; // click the LIF-198 row (global beat 43 at scene start 554)
const OPEN_START = 41; // list -> detail content crossfade begins
const OPEN_END = 53; // ...over ~12 frames
const BACK_CLICK = 138; // click the breadcrumb "Issues" link (global beat 50)
const CLOSE_START = 138; // detail -> list crossfade begins
const CLOSE_END = 147; // ...over ~9 frames
// Board beat — UNCHANGED from here on.
const VIEW_CLICK = 172; // cursor clicks "Board" (global beat 52 at scene start 548)
const SWITCH_START = 172; // list -> board content crossfade begins
const SWITCH_END = 184; // ...over ~12 frames
const DRAG_START = 229; // land 269 -> global 817 = beat 59
const DRAG_END = DRAG_START + 40; // 269 — lands beat-aligned
const CURSOR_EXIT = DRAG_END + 40; // 224

// Switcher pill "Board" segment center, APP-FRAME-local (calibrated
// against the f108 still). App-frame origin = LificApp top-left.
const SWITCH_BTN = { x: 439, y: 19 };

// LIF-198 row center, APP-FRAME-local (origin = LificApp top-left).
// Content panel starts at (SIDEBAR_W, TOPBAR_H); group order todo(3)/
// active/done, so LIF-198 is the 1st active row: header + 3 todo rows +
// active header stack above it. Row height = ROW_PY*2 + 16 icon.
const ROW_H = 36;
const rowCenterY = () =>
  TOPBAR_H + // content panel top
  GROUP_HEADER_H + // todo header
  3 * ROW_H + // 3 todo rows
  GROUP_HEADER_H + // active header
  ROW_H / 2; // half into LIF-198
const ROW_TARGET = { x: 500, y: rowCenterY() };

// Detail breadcrumb "Issues" link center, APP-FRAME-local. Detail topbar
// (y 0..TOPBAR_H) breadcrumb: content-area left is SIDEBAR_W, px-6 pad
// (24) then "LIF › Issues". "Issues" sits just past LIF + chevron.
const BACK_TARGET = { x: SIDEBAR_W + 24 + 56, y: TOPBAR_H / 2 };

const ease = Easing.bezier(0.4, 0, 0.2, 1);

export const WebUIScene: React.FC = () => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();

  // Whole-frame spring-in.
  const frameIn = spring({ frame, fps, config: { damping: 200, stiffness: 90 } });

  // ── Detail round-trip: open (list->detail) then close (detail->list) ──
  // detailT is 0 on the list, 1 on the detail view. It rises on the row
  // click and falls back on the "Issues" breadcrumb click, so the same
  // crossfade drives both legs.
  const detailT =
    frame < OPEN_START
      ? 0
      : frame < BACK_CLICK
        ? interpolate(frame, [OPEN_START, OPEN_END], [0, 1], {
            extrapolateLeft: "clamp",
            extrapolateRight: "clamp",
            easing: ease,
          })
        : interpolate(frame, [CLOSE_START, CLOSE_END], [1, 0], {
            extrapolateLeft: "clamp",
            extrapolateRight: "clamp",
            easing: ease,
          });
  const onDetail = frame >= OPEN_END - 1 && frame < CLOSE_END;

  // ── View switch: pill flip + content crossfade ──────────────
  const switchT = interpolate(frame, [SWITCH_START, SWITCH_END], [0, 1], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
    easing: ease,
  });
  const onBoard = frame >= SWITCH_END - 1;

  // Browser URL reflects the detail context between the two clicks.
  const url = onBoard
    ? "localhost:3456/#/LIF/board"
    : onDetail
      ? "localhost:3456/#/LIF/issue/LIF-198"
      : "localhost:3456/#/LIF/issues";

  // ── List row stagger-in (FAST: ~2-3 frames apart from f2) ───
  const rowReveal = (i: number) => {
    const s = spring({
      frame: frame - 2 - i * 2,
      fps,
      config: { damping: 200, stiffness: 130 },
    });
    return { opacity: s, dy: (1 - s) * 12 };
  };

  // ── Board drag mechanics (lifted from UIScene, faster) ──────
  const dragT = interpolate(frame, [DRAG_START, DRAG_END], [0, 1], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
    easing: ease,
  });
  const dragging = frame >= DRAG_START && frame < DRAG_END;
  const landed = frame >= DRAG_END;

  // Counts update at drop, exactly like the live app. Backlog stays a
  // hidden pill with 2 items; total label counts visible board issues.
  const counts = landed
    ? { backlog: 2, todo: 2, active: 3, done: 2 }
    : { backlog: 2, todo: 3, active: 2, done: 2 };

  // Content-panel-local source/destination for the dragged card.
  const srcX = colX(0) + CARD_PAD;
  const srcY = slotYAt(1);
  const dstX = colX(1) + CARD_PAD;
  const dstY = slotYAt(2);

  const settle = spring({
    frame: frame - DRAG_END,
    fps,
    config: { damping: 15, stiffness: 170, mass: 0.6 },
  });

  const movedPos = {
    x: srcX + (dstX - srcX) * dragT,
    y: srcY + (dstY - srcY) * dragT + Math.sin(dragT * Math.PI) * -18,
  };

  // ── Cursor script (APP-FRAME-local: origin = LificApp top-left) ──
  // The cursor overlays the WHOLE app frame, so board-card coords add the
  // sidebar width (content panel starts at SIDEBAR_W) horizontally and
  // TOPBAR_H (content panel starts below the topbar) vertically. The cursor
  // rides the card: grabs it ~150px in / +40 down at src, drops at dst.
  const grabX = SIDEBAR_W + srcX + 150;
  const grabY = TOPBAR_H + srcY + 40;
  const dropX = SIDEBAR_W + dstX + 150;
  const dropY = TOPBAR_H + dstY + 40;
  const CURSOR: Waypoint[] = [
    { at: 12, x: 470, y: 210 },
    // Move onto the LIF-198 row, then CLICK it (opens the detail view).
    { at: ROW_CLICK - 4, x: ROW_TARGET.x, y: ROW_TARGET.y },
    { at: ROW_CLICK, x: ROW_TARGET.x, y: ROW_TARGET.y, click: true },
    // Hold on the detail view, drift toward the "Issues" breadcrumb link.
    { at: BACK_CLICK - 8, x: BACK_TARGET.x, y: BACK_TARGET.y },
    { at: BACK_CLICK, x: BACK_TARGET.x, y: BACK_TARGET.y, click: true },
    // Back on the list — move up to the List|Board switcher, CLICK "Board".
    { at: VIEW_CLICK - 8, x: SWITCH_BTN.x, y: SWITCH_BTN.y },
    { at: VIEW_CLICK, x: SWITCH_BTN.x, y: SWITCH_BTN.y, click: true },
    // Drift down toward the card to be grabbed after the switch.
    { at: DRAG_START - 8, x: grabX, y: grabY - 4 },
    { at: DRAG_START, x: grabX, y: grabY, click: true },
    // Ride the card across to its destination.
    { at: DRAG_END, x: dropX, y: dropY },
    { at: DRAG_END + 8, x: dropX, y: dropY, click: true },
    { at: CURSOR_EXIT, x: dropX + 240, y: dropY + 200 },
  ];

  // Row-hover highlight: fade in as the cursor arrives on the LIF-198 row
  // (before the click), fade out as the detail opens. The hovered row is
  // the 4th flat index (0-based): LIF-231/214/207 (todo) then LIF-198.
  const HOVER_ROW = 3;
  const rowHover = interpolate(
    frame,
    [ROW_CLICK - 12, ROW_CLICK - 2, OPEN_START + 3, OPEN_START + 8],
    [0, 1, 1, 0],
    { extrapolateLeft: "clamp", extrapolateRight: "clamp" },
  );

  // ── Caption: fades in early, never leaves ───────────────────
  const captionIn = interpolate(frame, [6, 18], [0, 1], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
  });

  const framedScale = SCALE * (0.985 + frameIn * 0.015);

  return (
    <Background>
      <AbsoluteFill style={{ justifyContent: "center", alignItems: "center" }}>
        <div
          style={{
            transform: `scale(${framedScale})`,
            opacity: frameIn,
            marginTop: -40,
          }}
        >
          <BrowserFrame url={url} width={APP_W} height={APP_H + 52}>
            {/* Both frames share identical chrome; crossfading the whole
                app reads as the content swapping while the sidebar/topbar
                stay put. Both drive switchT so the pill flip is seamless. */}
            <div style={{ position: "relative", width: APP_W, height: APP_H }}>
              {/* LIST frame — fades under both the detail and the board. A
                  slight slide-left as the detail slides in from the right
                  (same style as the list->board crossfade). */}
              <div
                style={{
                  position: "absolute",
                  inset: 0,
                  opacity: (1 - switchT) * (1 - detailT),
                  transform: `translateX(${detailT * -20}px)`,
                }}
              >
                <IssueListPage
                  width={APP_W}
                  height={APP_H}
                  groups={LIST_GROUPS}
                  counts={{ backlog: 2, todo: 3, active: 2, done: 2 }}
                  totalLabel={String(TOTAL_ROWS)}
                  rowReveal={rowReveal}
                  switchT={switchT}
                  hoverIndex={HOVER_ROW}
                  hoverStrength={rowHover}
                />
              </div>

              {/* DETAIL frame — the LIF-198 round-trip. Slides in slightly
                  from the right, crossfading over the list. */}
              {detailT > 0 ? (
                <div
                  style={{
                    position: "absolute",
                    inset: 0,
                    opacity: detailT,
                    transform: `translateX(${(1 - detailT) * 24}px)`,
                  }}
                >
                  <IssueDetailPage
                    width={APP_W}
                    height={APP_H}
                    issue={DETAIL}
                  />
                </div>
              ) : null}

              {/* BOARD frame */}
              {switchT > 0 ? (
                <div
                  style={{
                    position: "absolute",
                    inset: 0,
                    opacity: switchT,
                  }}
                >
                  <LificApp
                    width={APP_W}
                    height={APP_H}
                    counts={counts}
                    totalLabel={"7"}
                    switchT={switchT}
                    sidebarActive="board"
                  >
                    {/* Drop-target outline on the hovered zone. */}
                    {dragging && dragT > 0.45 ? (
                      <div
                        style={{
                          position: "absolute",
                          left: colX(1) + 4,
                          top: PILLS_H + COL_HEADER_H + 4,
                          width: COL_W - 9,
                          bottom: 8,
                          outline: `2px dashed ${C.accent}`,
                          outlineOffset: -4,
                          borderRadius: 8,
                        }}
                      />
                    ) : null}

                    {/* Static cards. */}
                    {CARDS.filter((c) => c.issue.identifier !== MOVED).map((card) => {
                      // Only LIF-207 reflows (todo slot 2 -> 1) when the drag lifts.
                      let y = slotYAt(card.slot);
                      if (card.issue.identifier === "LIF-207") {
                        const s = spring({
                          frame: frame - (DRAG_START + 4),
                          fps,
                          config: { damping: 200, stiffness: 140 },
                        });
                        y =
                          frame < DRAG_START + 4
                            ? slotYAt(2)
                            : slotYAt(2) + (slotYAt(1) - slotYAt(2)) * s;
                      }
                      return (
                        <div
                          key={card.issue.identifier}
                          style={{
                            position: "absolute",
                            left: colX(card.col) + CARD_PAD,
                            top: y,
                          }}
                        >
                          <IssueCard issue={card.issue} width={CARD_W} />
                        </div>
                      );
                    })}

                    {/* The dragged card. */}
                    <div
                      style={{
                        position: "absolute",
                        left: movedPos.x,
                        top: movedPos.y,
                        zIndex: 30,
                        transform: dragging
                          ? "rotate(2deg) scale(1.02)"
                          : landed
                            ? `scale(${1 + (1 - settle) * 0.03})`
                            : undefined,
                        filter: dragging
                          ? "drop-shadow(0 14px 22px rgba(0,0,0,0.5))"
                          : undefined,
                      }}
                    >
                      <IssueCard
                        issue={CARDS.find((c) => c.issue.identifier === MOVED)!.issue}
                        width={CARD_W}
                      />
                    </div>
                  </LificApp>
                </div>
              ) : null}

              {/* Cursor overlays the whole app frame. */}
              <Cursor points={CURSOR} />
            </div>
          </BrowserFrame>
        </div>

        {/* Scrim so the caption never fights the app frame's bottom edge. */}
        <div
          style={{
            position: "absolute",
            left: 0,
            right: 0,
            bottom: 0,
            height: 240,
            background: `linear-gradient(to bottom, transparent, ${C.bg}ee 62%)`,
            opacity: captionIn,
          }}
        />
        {/* Caption — on screen for the whole scene. */}
        <div
          style={{
            position: "absolute",
            bottom: 40,
            width: "100%",
            textAlign: "center",
            fontFamily: BODY,
            opacity: captionIn,
            textShadow: "0 4px 30px rgba(0,0,0,0.9)",
          }}
        >
          <div style={{ fontSize: 44, fontWeight: 600, color: C.text, lineHeight: 1.1 }}>
            A real web UI. Everything you&rsquo;d expect.
          </div>
          <div style={{ fontSize: 30, color: C.textMuted, marginTop: 6, lineHeight: 1.2 }}>
            Issues, kanban, docs, plans, insights. All in the one binary.
          </div>
        </div>
      </AbsoluteFill>
    </Background>
  );
};

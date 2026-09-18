import React from "react";
import {
  AbsoluteFill,
  useCurrentFrame,
  useVideoConfig,
  spring,
  interpolate,
  Easing,
} from "remotion";
import { C } from "./theme";
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
} from "./components/lific-ui";
import { Cursor, Waypoint } from "./components/Cursor";

/*
 * BoardLoop — a seamless-loop clip for the lific.dev landing page.
 * The web UI cropped to just the board (sidebar + topbar cut away),
 * where "Create a landing page for lific.dev" gets dragged
 * todo -> active -> done. All motion is a pure function of
 * useCurrentFrame(); colors from theme C.
 *
 * Render: bunx remotion render BoardLoop ../site/public/board-loop.mp4
 */

// ── Issue world: shipping this very landing page ─────────────
const L: Record<string, Label> = {
  webui: { name: "web-ui", color: "#4dd9c7" },
  site: { name: "site", color: "#9287d7" },
};

const HERO: IssueData = {
  identifier: "LIF-240",
  title: "Create a landing page for lific.dev",
  priority: "high",
  labels: [L.site],
  updated: "now",
  status: "todo",
};

const OTHERS: { issue: IssueData; col: number; slot: number }[] = [
  {
    col: 0,
    slot: 1,
    issue: {
      identifier: "LIF-243",
      title: "OG image for social cards",
      priority: "low",
      labels: [L.site],
      updated: "1d ago",
      status: "todo",
    },
  },
  {
    col: 1,
    slot: 0,
    issue: {
      identifier: "LIF-238",
      title: "Sync landing copy with README",
      priority: "medium",
      labels: [L.site],
      updated: "3h ago",
      status: "active",
    },
  },
  {
    col: 2,
    slot: 0,
    issue: {
      identifier: "LIF-236",
      title: "Register lific.dev DNS",
      labels: [L.webui],
      updated: "2d ago",
      status: "done",
    },
  },
];

// ── Geometry ─────────────────────────────────────────────────
const APP_W = SIDEBAR_W + 916; // content panel 916 wide (3 cols + slack)
const APP_H = TOPBAR_H + 430; // content panel 430 tall (board crop)
const PANEL_W = APP_W - SIDEBAR_W;
const PANEL_H = APP_H - TOPBAR_H;
const SCALE = 2;

export const BOARD_LOOP_W = PANEL_W * SCALE; // 1832
export const BOARD_LOOP_H = PANEL_H * SCALE; // 1152
export const BOARD_LOOP_FRAMES = 240; // 8s @ 30fps

const CARD_H = 87; // single-line card
const PITCH = CARD_H + 8;
const slotY = (slot: number) => cardsTop() + slot * PITCH;

// ── Beat table ───────────────────────────────────────────────
const GRAB_1 = 48;
const DROP_1 = 82;
const GRAB_2 = 142;
const DROP_2 = 176;
const FADE_OUT = 228;

const ease = Easing.bezier(0.4, 0, 0.2, 1);

export const BoardLoop: React.FC = () => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();

  // ── Drag legs ──────────────────────────────────────────────
  const drag1 = interpolate(frame, [GRAB_1, DROP_1], [0, 1], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
    easing: ease,
  });
  const drag2 = interpolate(frame, [GRAB_2, DROP_2], [0, 1], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
    easing: ease,
  });
  const dragging =
    (frame >= GRAB_1 && frame < DROP_1) || (frame >= GRAB_2 && frame < DROP_2);

  // Hero card position: todo slot0 -> active slot1 -> done slot1.
  const p0 = { x: colX(0) + CARD_PAD, y: slotY(0) };
  const p1 = { x: colX(1) + CARD_PAD, y: slotY(1) };
  const p2 = { x: colX(2) + CARD_PAD, y: slotY(1) };
  const leg = frame < GRAB_2 ? drag1 : drag2;
  const from = frame < GRAB_2 ? p0 : p1;
  const to = frame < GRAB_2 ? p1 : p2;
  const heroPos = {
    x: from.x + (to.x - from.x) * leg,
    y: from.y + (to.y - from.y) * leg + Math.sin(leg * Math.PI) * -16,
  };

  // Settle bounce after each drop.
  const settle1 = spring({
    frame: frame - DROP_1,
    fps,
    config: { damping: 15, stiffness: 170, mass: 0.6 },
  });
  const settle2 = spring({
    frame: frame - DROP_2,
    fps,
    config: { damping: 15, stiffness: 170, mass: 0.6 },
  });
  const settle = frame < GRAB_2 ? settle1 : settle2;
  const landed = (frame >= DROP_1 && frame < GRAB_2) || frame >= DROP_2;

  // Status flips at each drop; counts follow, like the live app.
  const heroStatus =
    frame < DROP_1 ? "todo" : frame < DROP_2 ? "active" : "done";
  const counts =
    frame < DROP_1
      ? { backlog: 1, todo: 2, active: 1, done: 1 }
      : frame < DROP_2
        ? { backlog: 1, todo: 1, active: 2, done: 1 }
        : { backlog: 1, todo: 1, active: 1, done: 2 };

  // Done celebration: a green glow that blooms on the second drop.
  const doneGlow =
    frame >= DROP_2
      ? interpolate(frame, [DROP_2, DROP_2 + 6, DROP_2 + 30], [0, 1, 0], {
          extrapolateLeft: "clamp",
          extrapolateRight: "clamp",
        })
      : 0;

  // LIF-243 reflows todo slot1 -> slot0 once the hero lifts away.
  const reflow = spring({
    frame: frame - (GRAB_1 + 6),
    fps,
    config: { damping: 200, stiffness: 140 },
  });
  const lif243Y =
    frame < GRAB_1 + 6 ? slotY(1) : slotY(1) + (slotY(0) - slotY(1)) * reflow;

  // Card stagger-in at the top of the loop.
  const cardIn = (i: number) => {
    const s = spring({
      frame: frame - 4 - i * 3,
      fps,
      config: { damping: 200, stiffness: 120 },
    });
    return { opacity: s, dy: (1 - s) * 14 };
  };

  // Drop-target outline on the hovered column while dragging.
  const targetCol = frame < GRAB_2 ? 1 : 2;
  const showTarget = dragging && leg > 0.4;

  // ── Cursor script (content-panel-local coordinates) ────────
  const grip = { dx: 150, dy: 42 }; // where the pointer holds the card
  const CURSOR: Waypoint[] = [
    { at: 16, x: colX(1) + 40, y: 320 },
    { at: GRAB_1 - 6, x: p0.x + grip.dx, y: p0.y + grip.dy },
    { at: GRAB_1, x: p0.x + grip.dx, y: p0.y + grip.dy, click: true },
    { at: DROP_1, x: p1.x + grip.dx, y: p1.y + grip.dy },
    { at: DROP_1 + 6, x: p1.x + grip.dx, y: p1.y + grip.dy, click: true },
    // Drift while the first drop settles, then return for leg two.
    { at: 116, x: p1.x + grip.dx + 60, y: p1.y + grip.dy + 90 },
    { at: GRAB_2 - 6, x: p1.x + grip.dx, y: p1.y + grip.dy },
    { at: GRAB_2, x: p1.x + grip.dx, y: p1.y + grip.dy, click: true },
    { at: DROP_2, x: p2.x + grip.dx, y: p2.y + grip.dy },
    { at: DROP_2 + 6, x: p2.x + grip.dx, y: p2.y + grip.dy, click: true },
    { at: DROP_2 + 34, x: p2.x + grip.dx + 130, y: p2.y + grip.dy + 170 },
  ];

  // Loop seam: fade to the board floor at the very end so the restart
  // (with its stagger-in) reads as a cut, not a jump.
  const fade = interpolate(frame, [FADE_OUT, BOARD_LOOP_FRAMES - 2], [0, 1], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
  });

  return (
    <AbsoluteFill style={{ backgroundColor: C.bg, overflow: "hidden" }}>
      {/* The app frame, scaled and offset so only the content panel
          (pills + board columns) is in view. */}
      <div
        style={{
          position: "absolute",
          left: -SIDEBAR_W * SCALE,
          top: -TOPBAR_H * SCALE,
          transform: `scale(${SCALE})`,
          transformOrigin: "top left",
        }}
      >
        <LificApp
          width={APP_W}
          height={APP_H}
          counts={counts}
          totalLabel={String(4)}
          sidebarActive="board"
        >
          {/* Drop-target outline */}
          {showTarget ? (
            <div
              style={{
                position: "absolute",
                left: colX(targetCol) + 4,
                top: PILLS_H + COL_HEADER_H + 4,
                width: COL_W - 9,
                bottom: 8,
                outline: `2px dashed ${C.accent}`,
                outlineOffset: -4,
                borderRadius: 8,
              }}
            />
          ) : null}

          {/* Static cards */}
          {OTHERS.map((card, i) => {
            const inAnim = cardIn(i + 1);
            const y =
              card.issue.identifier === "LIF-243" ? lif243Y : slotY(card.slot);
            return (
              <div
                key={card.issue.identifier}
                style={{
                  position: "absolute",
                  left: colX(card.col) + CARD_PAD,
                  top: y + inAnim.dy,
                  opacity: inAnim.opacity,
                }}
              >
                <IssueCard issue={card.issue} width={CARD_W} />
              </div>
            );
          })}

          {/* The hero card */}
          <div
            style={{
              position: "absolute",
              left: heroPos.x,
              top: heroPos.y + cardIn(0).dy,
              opacity: cardIn(0).opacity,
              zIndex: 30,
              transform: dragging
                ? "rotate(2deg) scale(1.02)"
                : landed
                  ? `scale(${1 + (1 - settle) * 0.03})`
                  : undefined,
              filter: dragging
                ? "drop-shadow(0 14px 22px rgba(0,0,0,0.5))"
                : doneGlow > 0
                  ? `drop-shadow(0 0 ${14 * doneGlow}px ${C.success}88)`
                  : undefined,
            }}
          >
            <IssueCard issue={{ ...HERO, status: heroStatus }} width={CARD_W} />
          </div>

          {/* Cursor rides inside the content panel. */}
          <Cursor points={CURSOR} />
        </LificApp>
      </div>

      {/* Loop-seam fade */}
      <AbsoluteFill
        style={{ backgroundColor: C.bg, opacity: fade, pointerEvents: "none" }}
      />
    </AbsoluteFill>
  );
};

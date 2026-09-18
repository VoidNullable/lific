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
} from "../components/lific-ui";
import { Cursor, Waypoint } from "../components/Cursor";

/*
 * Pixel-faithful board demo, zoomed for phone legibility: 3 visible
 * columns (Backlog/Cancelled shown as hidden pills, like the real
 * Columns control), one drag with svelte-dnd-action's dashed accent
 * drop outline, live count updates, column reflow.
 */

const L: Record<string, Label> = {
  webui: { name: "web-ui", color: "#4dd9c7" },
  core: { name: "core", color: "#9287d7" },
  mcp: { name: "mcp", color: "#b48af0" },
  auth: { name: "auth", color: "#fb923c" },
  bug: { name: "bug", color: "#f87171" },
};

type BoardCard = {
  issue: IssueData;
  col: number; // 0 todo, 1 active, 2 done
  slot: number;
};

const CARDS: BoardCard[] = [
  {
    col: 0,
    slot: 0,
    issue: {
      identifier: "LIF-231",
      title: "Board column virtualization",
      priority: "medium",
      labels: [L.webui],
      updated: "2d ago",
    },
  },
  {
    col: 0,
    slot: 1,
    issue: {
      identifier: "LIF-214",
      title: "Bulk-edit issues from the list",
      priority: "high",
      labels: [L.webui],
      updated: "4h ago",
    },
  },
  {
    col: 0,
    slot: 2,
    issue: {
      identifier: "LIF-207",
      title: "Saved filters per project",
      priority: "low",
      updated: "1d ago",
    },
  },
  {
    col: 1,
    slot: 0,
    issue: {
      identifier: "LIF-198",
      title: "Fix WAL checkpoint race",
      priority: "high",
      labels: [L.core, L.bug],
      updated: "26m ago",
    },
  },
  {
    col: 1,
    slot: 1,
    issue: {
      identifier: "LIF-226",
      title: "MCP: recurring plan templates",
      priority: "medium",
      labels: [L.mcp],
      updated: "2h ago",
    },
  },
  {
    col: 2,
    slot: 0,
    issue: {
      identifier: "LIF-183",
      title: "OAuth device flow for CLI",
      labels: [L.auth],
      updated: "5h ago",
      status: "done",
    },
  },
  {
    col: 2,
    slot: 1,
    issue: {
      identifier: "LIF-171",
      title: "Backup retention config",
      labels: [L.core],
      updated: "1d ago",
      status: "done",
    },
  },
];

// Deterministic card metrics (single-line titles): 87px card, 8px gap.
const CARD_H = 87;
const PITCH = CARD_H + 8;
const slotYAt = (slot: number) => cardsTop() + slot * PITCH;

// Drag: LIF-214 (todo slot 1) -> active slot 2. In Ad A the grab lands
// on the track's second drop (bar 17, global 886) with dragStart=75.
const MOVED = "LIF-214";

const ease = Easing.bezier(0.4, 0, 0.2, 1);

export const UIScene: React.FC<{ dragStart?: number }> = ({
  dragStart = 75,
}) => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();
  const DRAG_START = dragStart;
  const DRAG_END = DRAG_START + 50;

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

  const CURSOR: Waypoint[] = [
    { at: 14, x: colX(2) + 220, y: 500 },
    { at: DRAG_START - 7, x: srcX + 150, y: srcY + 40 },
    { at: DRAG_START, x: srcX + 150, y: srcY + 40, click: true },
    { at: DRAG_END, x: dstX + 150, y: dstY + 40 },
    { at: DRAG_END + 8, x: dstX + 150, y: dstY + 40, click: true },
    { at: DRAG_END + 45, x: dstX + 260, y: dstY + 220 },
  ];

  const frameIn = spring({
    frame,
    fps,
    config: { damping: 200, stiffness: 90 },
  });
  const captionIn = interpolate(frame, [20, 36], [0, 1], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
  });

  // Native app size (3 columns), zoomed hard for phone-in-feed legibility.
  const APP_W = 1146;
  const APP_H = 620;
  const SCALE = 1.42;

  return (
    <Background>
      <AbsoluteFill style={{ justifyContent: "center", alignItems: "center" }}>
        <div
          style={{
            transform: `scale(${SCALE * (0.985 + frameIn * 0.015)})`,
            opacity: frameIn,
            marginTop: -40,
          }}
        >
          <BrowserFrame
            url="localhost:3456/#/LIF/board"
            width={APP_W}
            height={APP_H + 52}
          >
            <LificApp
              width={APP_W}
              height={APP_H}
              counts={counts}
              totalLabel={"7"}
            >
              {/* svelte-dnd-action drop-target outline on the hovered zone */}
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

              {/* Static cards */}
              {CARDS.filter((c) => c.issue.identifier !== MOVED).map((card) => {
                // Only LIF-207 reflows (todo slot 2 -> 1) when the drag lifts.
                let y = slotYAt(card.slot);
                if (card.issue.identifier === "LIF-207") {
                  const s = spring({
                    frame: frame - (DRAG_START + 6),
                    fps,
                    config: { damping: 200, stiffness: 140 },
                  });
                  y =
                    frame < DRAG_START + 6
                      ? slotYAt(2)
                      : slotYAt(2) + (slotYAt(1) - slotYAt(2)) * s;
                }
                const enter = spring({
                  frame: frame - 4 - (card.col * 2 + card.slot) * 2,
                  fps,
                  config: { damping: 200, stiffness: 120 },
                });
                return (
                  <div
                    key={card.issue.identifier}
                    style={{
                      position: "absolute",
                      left: colX(card.col) + CARD_PAD,
                      top: y,
                      opacity: enter,
                      transform: `translateY(${(1 - enter) * 14}px)`,
                    }}
                  >
                    <IssueCard issue={card.issue} width={CARD_W} />
                  </div>
                );
              })}

              {/* The dragged card */}
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
                  opacity: spring({
                    frame: frame - 8,
                    fps,
                    config: { damping: 200, stiffness: 120 },
                  }),
                }}
              >
                <IssueCard
                  issue={CARDS.find((c) => c.issue.identifier === MOVED)!.issue}
                  width={CARD_W}
                />
              </div>

              <Cursor points={CURSOR} />
            </LificApp>
          </BrowserFrame>
        </div>

        <div
          style={{
            position: "absolute",
            bottom: 26,
            fontFamily: BODY,
            fontSize: 42,
            fontWeight: 500,
            color: C.text,
            opacity: captionIn,
            textShadow: "0 4px 30px rgba(0,0,0,0.9)",
          }}
        >
          Issues, kanban, pages, modules.{" "}
          <span style={{ color: C.textMuted }}>
            The whole tracker, no seat math.
          </span>
        </div>
      </AbsoluteFill>
    </Background>
  );
};

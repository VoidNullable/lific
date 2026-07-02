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
import { BODY, DISPLAY } from "../fonts";
import { Background } from "../components/Background";
import { BrowserFrame } from "../components/BrowserFrame";
import { AppShell, IssueCard, CardData } from "../components/board";
import { Cursor, Waypoint } from "../components/Cursor";

/*
 * Board demo: two real drag-and-drops (Todo -> Active, Active -> Done),
 * cursor-choreographed. Every card position is a pure function of frame.
 */

const CARD_W = 330;
const CARD_H = 92;
const PITCH = CARD_H + 12;
const COL_X = [44, 44 + 366, 44 + 732];
const CARD_TOP = 84;

const slotPos = (col: number, slot: number) => ({
  x: COL_X[col],
  y: CARD_TOP + slot * PITCH,
});

const ease = Easing.bezier(0.4, 0, 0.2, 1);

type Keyframe = { at: number; col: number; slot: number; drag?: boolean };

type BoardCard = { data: CardData; frames: Keyframe[] };

// Drag 1: LIF-214 todo#1 -> active#2 over frames 70..140
// Drag 2: LIF-198 active#0 -> done#0 over frames 200..270
const CARDS: BoardCard[] = [
  {
    data: { id: "LIF-231", title: "Board column virtualization", priority: "medium", module: "Web UI" },
    frames: [{ at: 0, col: 0, slot: 0 }],
  },
  {
    data: { id: "LIF-214", title: "Bulk-edit issues from the list view", priority: "high", module: "Web UI" },
    frames: [
      { at: 0, col: 0, slot: 1 },
      { at: 70, col: 1, slot: 2, drag: true },
    ],
  },
  {
    data: { id: "LIF-207", title: "Saved filters per project", priority: "low" },
    frames: [
      { at: 0, col: 0, slot: 2 },
      { at: 90, col: 0, slot: 1 },
    ],
  },
  {
    data: { id: "LIF-198", title: "Fix WAL checkpoint race on shutdown", priority: "high", module: "Core" },
    frames: [
      { at: 0, col: 1, slot: 0 },
      { at: 200, col: 2, slot: 0, drag: true },
    ],
  },
  {
    data: { id: "LIF-226", title: "MCP: recurring plan templates", priority: "medium", module: "MCP" },
    frames: [
      { at: 0, col: 1, slot: 1 },
      { at: 220, col: 1, slot: 0 },
    ],
  },
  {
    data: { id: "LIF-183", title: "OAuth device flow for CLI login", module: "Auth" },
    frames: [
      { at: 0, col: 2, slot: 0 },
      { at: 250, col: 2, slot: 1 },
    ],
  },
  {
    data: { id: "LIF-171", title: "Backup retention config", module: "Core" },
    frames: [
      { at: 0, col: 2, slot: 1 },
      { at: 250, col: 2, slot: 2 },
    ],
  },
];

const DRAG_DUR = 70;

const cardTransform = (frame: number, fps: number, card: BoardCard) => {
  const kfs = card.frames;
  let cur = kfs[0];
  for (let i = kfs.length - 1; i >= 0; i--) {
    if (frame >= kfs[i].at) {
      cur = kfs[i];
      break;
    }
  }
  // Before first keyframe.
  if (frame < kfs[0].at) {
    const p = slotPos(kfs[0].col, kfs[0].slot);
    return { ...p, rotate: 0, scale: 1, lifted: false, arriveFlash: 0 };
  }

  const from = kfs.indexOf(cur) === 0 ? cur : kfs[kfs.indexOf(cur) - 1];
  const src = slotPos(from.col, from.slot);
  const dst = slotPos(cur.col, cur.slot);

  if (cur.drag) {
    const t = interpolate(frame, [cur.at, cur.at + DRAG_DUR], [0, 1], {
      extrapolateLeft: "clamp",
      extrapolateRight: "clamp",
      easing: ease,
    });
    const lifted = t > 0 && t < 1;
    // Settle spring at the end of the drag.
    const settle = spring({
      frame: frame - (cur.at + DRAG_DUR),
      fps,
      config: { damping: 14, stiffness: 160, mass: 0.7 },
    });
    const rotate = lifted ? Math.sin(t * Math.PI) * 3 : 0;
    const scale = lifted ? 1.05 : frame >= cur.at + DRAG_DUR ? 1 + (1 - settle) * 0.05 : 1;
    const arc = Math.sin(t * Math.PI) * -26;
    const arriveFlash =
      frame >= cur.at + DRAG_DUR
        ? Math.max(0, 1 - (frame - (cur.at + DRAG_DUR)) / 24)
        : 0;
    return {
      x: src.x + (dst.x - src.x) * t,
      y: src.y + (dst.y - src.y) * t + arc,
      rotate,
      scale,
      lifted,
      arriveFlash: cur.col === 2 ? arriveFlash : 0,
    };
  }

  // Reflow shift: spring between previous slot and new slot.
  const s = spring({
    frame: frame - cur.at,
    fps,
    config: { damping: 200, stiffness: 130 },
  });
  return {
    x: src.x + (dst.x - src.x) * s,
    y: src.y + (dst.y - src.y) * s,
    rotate: 0,
    scale: 1,
    lifted: false,
    arriveFlash: 0,
  };
};

// Cursor grabs card centers. Card center offset ~ (165, 46).
const grab = (col: number, slot: number) => {
  const p = slotPos(col, slot);
  return { x: p.x + 165, y: p.y + 46 };
};

const CURSOR: Waypoint[] = [
  { at: 20, x: 1120, y: 640 },
  { at: 62, ...grab(0, 1) },
  { at: 70, ...grab(0, 1), click: true },
  { at: 70 + DRAG_DUR, ...grab(1, 2) },
  { at: 150, ...grab(1, 2), click: true },
  { at: 192, ...grab(1, 0) },
  { at: 200, ...grab(1, 0), click: true },
  { at: 200 + DRAG_DUR, ...grab(2, 0) },
  { at: 280, ...grab(2, 0), click: true },
  { at: 330, x: 1180, y: 700 },
];

const COLUMNS = [
  { name: "Todo", dot: C.textFaint },
  { name: "Active", dot: C.warn },
  { name: "Done", dot: C.success },
];

const colCount = (frame: number, col: number) =>
  CARDS.filter((c) => {
    let cur = c.frames[0];
    for (const kf of c.frames) if (frame >= kf.at) cur = kf;
    return cur.col === col;
  }).length;

export const UIScene: React.FC = () => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();

  const frameIn = spring({ frame, fps, config: { damping: 200, stiffness: 80 } });
  const captionIn = interpolate(frame, [290, 312], [0, 1], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
  });

  return (
    <Background>
      <AbsoluteFill style={{ justifyContent: "center", alignItems: "center" }}>
        <div
          style={{
            transform: `scale(${0.96 + frameIn * 0.04}) translateY(${(1 - frameIn) * 40 - 24}px)`,
            opacity: frameIn,
          }}
        >
          <BrowserFrame url="localhost:8080/projects/LIF/board" width={1560} height={840}>
            <AppShell title="Board" active="Board">
              {/* Column headers */}
              <div style={{ position: "absolute", inset: 0 }}>
                {COLUMNS.map((c, i) => (
                  <div
                    key={c.name}
                    style={{
                      position: "absolute",
                      left: COL_X[i],
                      top: 30,
                      display: "flex",
                      alignItems: "center",
                      gap: 9,
                    }}
                  >
                    <span
                      style={{
                        width: 10,
                        height: 10,
                        borderRadius: 5,
                        backgroundColor: c.dot,
                      }}
                    />
                    <span
                      style={{
                        fontFamily: DISPLAY,
                        fontSize: 17,
                        fontWeight: 600,
                        color: C.text,
                      }}
                    >
                      {c.name}
                    </span>
                    <span style={{ fontFamily: BODY, fontSize: 14, color: C.textFaint }}>
                      {colCount(frame, i)}
                    </span>
                  </div>
                ))}

                {/* Cards */}
                {CARDS.map((card, i) => {
                  const enter = spring({
                    frame: frame - 6 - i * 3,
                    fps,
                    config: { damping: 200, stiffness: 110 },
                  });
                  const t = cardTransform(frame, fps, card);
                  return (
                    <div
                      key={card.data.id}
                      style={{
                        position: "absolute",
                        left: t.x,
                        top: t.y,
                        opacity: enter,
                        transform: `translateY(${(1 - enter) * 24}px) rotate(${t.rotate}deg) scale(${t.scale})`,
                        zIndex: t.lifted ? 40 : 10,
                        filter: t.lifted
                          ? "drop-shadow(0 18px 30px rgba(0,0,0,0.5))"
                          : undefined,
                      }}
                    >
                      <IssueCard
                        card={card.data}
                        width={CARD_W}
                        highlight={t.arriveFlash}
                        style={{ height: CARD_H, overflow: "hidden" }}
                      />
                    </div>
                  );
                })}

                <Cursor points={CURSOR} />
              </div>
            </AppShell>
          </BrowserFrame>
        </div>

        <div
          style={{
            position: "absolute",
            bottom: 44,
            fontFamily: BODY,
            fontSize: 36,
            fontWeight: 500,
            color: C.text,
            opacity: captionIn,
          }}
        >
          Issues, kanban, pages, modules —{" "}
          <span style={{ color: C.textMuted }}>the whole tracker, no seat math.</span>
        </div>
      </AbsoluteFill>
    </Background>
  );
};

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
import { BODY, MONO } from "./fonts";
import { History } from "./components/icons";
import {
  IssueCard,
  IssueData,
  Label,
  ColumnHeader,
  StatusIcon,
  CARD_PAD,
} from "./components/lific-ui";
import { Cursor, Waypoint, cursorPos } from "./components/Cursor";
import { TUI, ToolLine, Typed, SessionChip } from "./PlanSync";

/*
 * EveryoneSplit: landing-page loop for the "For everyone" section.
 * Left: an agent session picks up APP-43 over MCP. Right: the same
 * board in the web UI, where APP-43 slides to Active on its own (the
 * realtime update) while blake drags APP-51 to Done by hand. Under
 * both, the project activity feed records each change as it lands,
 * with who did it and through which door.
 *
 * Render: bunx remotion render EveryoneSplit ../site/public/everyone-split.mp4 --muted
 * Poster: bunx remotion still EveryoneSplit /tmp/opencode/everyone-poster.png --frame=150
 *         magick /tmp/opencode/everyone-poster.png -quality 82 ../site/public/everyone-poster.webp
 */

// ── Geometry ─────────────────────────────────────────────────
const S = 1.45; // board + feed render at app CSS px, scaled up
const COL = 236; // narrower than the app's 300 so three columns fit
const CARD_W = COL - CARD_PAD * 2 - 1;
const BOARD_W = COL * 3; // 708 CSS px
const BOARD_H = 345; // header + three card slots
const HEADER_H = 40;
const CARD_H = 87;
const PITCH = CARD_H + 8;
const slotY = (slot: number) => HEADER_H + CARD_PAD + slot * PITCH;
const colX = (i: number) => i * COL;

const MX = 36; // outer margin
const TOP = 30;
const GAP = 28;
const RIGHT_W = Math.round(BOARD_W * S) + 2;
const PANE_H = Math.round(BOARD_H * S) + 2;
const LEFT_W = 1832 - MX * 2 - GAP - RIGHT_W;
const STRIP_TOP = TOP + PANE_H + 20;
const FEED_W = (1832 - MX * 2) / S;
const ROW_H = 32;

export const EVERYONE_SPLIT_W = 1832;
export const EVERYONE_SPLIT_H = 740;
export const EVERYONE_SPLIT_FRAMES = 240; // 8s @ 30fps

// ── Beat table (30fps) ───────────────────────────────────────
const T0 = 8; // get_plan tool line
const GRAB = 34; // blake picks up APP-51
const R1 = 28; // typed reply
const T1 = 60; // update_issue tool line (check lands ~T1+12)
const DROP = 70; // APP-51 lands in Done
const MOVE = 76; // realtime: APP-43 slides to Active
const MOVE_END = MOVE + 18;
const FEED_B = DROP + 4; // blake's feed row
const FEED_O = MOVE_END; // opencode-blake's feed row
const FADE = 222; // loop-seam fade

const ease = Easing.bezier(0.4, 0, 0.2, 1);
const clamp = {
  extrapolateLeft: "clamp",
  extrapolateRight: "clamp",
} as const;

// ── Issue world: the offline-sync project from PlanSync ──────
const L: Record<string, Label> = {
  sync: { name: "sync", color: "#4dd9c7" },
  launch: { name: "launch", color: "#9287d7" },
  ops: { name: "ops", color: "#e0a458" },
};

const APP43: IssueData = {
  identifier: "APP-43",
  title: "Retry with exponential backoff",
  priority: "high",
  labels: [L.sync],
  updated: "1d ago",
};
const APP51: IssueData = {
  identifier: "APP-51",
  title: "Pick the launch date",
  priority: "medium",
  labels: [L.launch],
  updated: "2h ago",
};
const APP52: IssueData = {
  identifier: "APP-52",
  title: "Sign the vendor contract",
  priority: "low",
  labels: [L.ops],
  updated: "3d ago",
  status: "todo",
};
const APP48: IssueData = {
  identifier: "APP-48",
  title: "Sync status in the header",
  priority: "medium",
  labels: [L.sync],
  updated: "40m ago",
  status: "active",
};
const APP42: IssueData = {
  identifier: "APP-42",
  title: "Conflict resolution",
  labels: [L.sync],
  updated: "5h ago",
  status: "done",
};

/** Small, quiet video label naming who made a change. */
const Tag: React.FC<{
  children: React.ReactNode;
  agent?: boolean;
  opacity: number;
  style?: React.CSSProperties;
}> = ({ children, agent, opacity, style }) => (
  <div
    style={{
      position: "absolute",
      zIndex: 60,
      fontFamily: MONO,
      fontSize: 11,
      lineHeight: "16px",
      padding: "1px 7px",
      borderRadius: 999,
      whiteSpace: "nowrap",
      color: agent ? C.accent : C.text,
      backgroundColor: agent ? C.accentSubtle : C.chrome,
      border: `1px solid ${agent ? `${C.accent}66` : C.border}`,
      boxShadow: "0 2px 6px rgba(0,0,0,0.35)",
      opacity,
      ...style,
    }}
  >
    {children}
  </div>
);

// ── Activity feed row (ProjectActivity.svelte + ActivityTimeline) ──
const Avatar: React.FC<{ letter: string; bot?: boolean }> = ({ letter, bot }) => (
  <span
    style={{
      width: 24,
      height: 24,
      borderRadius: 12,
      flexShrink: 0,
      display: "flex",
      alignItems: "center",
      justifyContent: "center",
      boxSizing: "border-box",
      fontSize: 11,
      fontWeight: 700,
      backgroundColor: bot ? C.accentSubtle : C.accent,
      color: bot ? C.accent : C.stone950,
      border: bot ? `1px solid ${C.accent}` : "none",
    }}
  >
    {letter}
  </span>
);

const AgentBadge: React.FC = () => (
  <span
    style={{
      display: "inline-block",
      verticalAlign: "middle",
      fontSize: 11,
      fontWeight: 600,
      textTransform: "uppercase",
      letterSpacing: "0.05em",
      padding: "1px 4px",
      borderRadius: 4,
      backgroundColor: C.accentSubtle,
      color: C.accent,
      margin: "0 2px",
    }}
  >
    agent
  </span>
);

const StatusValue: React.FC<{ status: string; strong?: boolean }> = ({
  status,
  strong,
}) => (
  <span
    style={{
      display: "inline-flex",
      alignItems: "center",
      gap: 4,
      verticalAlign: "middle",
      margin: "0 2px",
    }}
  >
    <StatusIcon status={status} size={12} />
    <span style={{ textTransform: "capitalize", color: strong ? C.text : undefined }}>
      {status}
    </span>
  </span>
);

const FeedRow: React.FC<{
  letter: string;
  actor: string;
  bot?: boolean;
  issue: string;
  from: string;
  to: string;
  transport: string;
  y: number;
  opacity: number;
}> = ({ letter, actor, bot, issue, from, to, transport, y, opacity }) => (
  <div
    style={{
      position: "absolute",
      left: 0,
      right: 0,
      top: y,
      height: ROW_H,
      display: "flex",
      alignItems: "center",
      gap: 10,
      padding: "0 10px",
      opacity,
      fontSize: 13,
      color: C.textMuted,
      whiteSpace: "nowrap",
    }}
  >
    <Avatar letter={letter} bot={bot} />
    <div>
      <span style={{ fontWeight: 500, color: C.text }}>{actor}</span>
      {bot ? <AgentBadge /> : " "}
      changed status{" "}
      <span style={{ fontFamily: MONO, fontSize: 12, color: C.accent }}>{issue}</span>{" "}
      <StatusValue status={from} />
      <span style={{ color: C.textFaint }}>&rarr;</span>
      <StatusValue status={to} strong />
      <span style={{ fontSize: 12, color: C.textFaint }}>
        {" "}
        &middot; just now via {transport}
      </span>
    </div>
  </div>
);

export const EveryoneSplit: React.FC = () => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();

  const panelIn = spring({ frame, fps, config: { damping: 200, stiffness: 90 } });
  const lift = (1 - panelIn) * 20;

  // ── blake drags APP-51: todo slot1 -> done slot1 ───────────
  const drag = interpolate(frame, [GRAB, DROP], [0, 1], { ...clamp, easing: ease });
  const dragging = frame >= GRAB && frame < DROP;
  const a0 = { x: colX(0) + CARD_PAD, y: slotY(1) };
  const a1 = { x: colX(2) + CARD_PAD, y: slotY(1) };
  const pos51 = {
    x: a0.x + (a1.x - a0.x) * drag,
    y: a0.y + (a1.y - a0.y) * drag + Math.sin(drag * Math.PI) * -16,
  };
  const settle = spring({
    frame: frame - DROP,
    fps,
    config: { damping: 15, stiffness: 170, mass: 0.6 },
  });
  const doneGlow =
    frame >= DROP
      ? interpolate(frame, [DROP, DROP + 6, DROP + 30], [0, 1, 0], clamp)
      : 0;
  const showTarget = dragging && drag > 0.4;

  // ── Realtime: APP-43 slides todo slot0 -> active slot1 ─────
  const move = interpolate(frame, [MOVE, MOVE_END], [0, 1], { ...clamp, easing: ease });
  const b0 = { x: colX(0) + CARD_PAD, y: slotY(0) };
  const b1 = { x: colX(1) + CARD_PAD, y: slotY(1) };
  const pos43 = {
    x: b0.x + (b1.x - b0.x) * move,
    y: b0.y + (b1.y - b0.y) * move,
  };
  const hl43 =
    frame >= MOVE ? interpolate(frame, [MOVE, MOVE + 6, MOVE + 54], [0, 1, 0], clamp) : 0;

  // APP-52 closes each gap it is left with: slot2 -> slot1 -> slot0.
  const reflow1 = spring({ frame: frame - (GRAB + 6), fps, config: { damping: 200, stiffness: 140 } });
  const reflow2 = spring({ frame: frame - (MOVE + 8), fps, config: { damping: 200, stiffness: 140 } });
  const y52 =
    slotY(2) +
    (frame >= GRAB + 6 ? (slotY(1) - slotY(2)) * reflow1 : 0) +
    (frame >= MOVE + 8 ? (slotY(0) - slotY(1)) * reflow2 : 0);

  const counts = {
    todo: 3 - (frame >= DROP ? 1 : 0) - (frame >= MOVE ? 1 : 0),
    active: 1 + (frame >= MOVE ? 1 : 0),
    done: 1 + (frame >= DROP ? 1 : 0),
  };

  // ── Cursor (board CSS coordinates) ─────────────────────────
  const grip = { dx: 140, dy: 62 }; // low on the card, so the tag clears it
  const CURSOR: Waypoint[] = [
    { at: 0, x: colX(1) + 110, y: 300 },
    { at: GRAB - 6, x: a0.x + grip.dx, y: a0.y + grip.dy },
    { at: GRAB, x: a0.x + grip.dx, y: a0.y + grip.dy, click: true },
    { at: DROP, x: a1.x + grip.dx, y: a1.y + grip.dy },
    { at: DROP + 6, x: a1.x + grip.dx, y: a1.y + grip.dy, click: true },
    { at: DROP + 44, x: a1.x + 90, y: 300 },
  ];
  const cur = cursorPos(frame, CURSOR);
  const blakeTag = interpolate(frame, [4, 12, DROP + 30, DROP + 42], [0, 1, 1, 0], clamp);
  const agentTag = interpolate(frame, [MOVE + 12, MOVE + 20, MOVE + 80, MOVE + 92], [0, 1, 1, 0], clamp);

  // ── Feed: newest first, so blake's row slides down when the
  //    agent's change lands on top of it.
  const inB = interpolate(frame, [FEED_B, FEED_B + 8], [0, 1], clamp);
  const inO = interpolate(frame, [FEED_O, FEED_O + 8], [0, 1], clamp);
  const push = spring({ frame: frame - FEED_O, fps, config: { damping: 200, stiffness: 140 } });
  const rowB = (frame >= FEED_O ? push : 0) * ROW_H;
  const feedCount = (frame >= FEED_B ? 1 : 0) + (frame >= FEED_O ? 1 : 0);

  const fade = interpolate(frame, [FADE, EVERYONE_SPLIT_FRAMES - 2], [0, 1], clamp);

  const pane: React.CSSProperties = {
    position: "absolute",
    top: TOP,
    height: PANE_H,
    boxSizing: "border-box",
    borderRadius: 16,
    border: `1px solid ${C.border}`,
    boxShadow: "0 30px 80px rgba(0,0,0,0.55)",
    overflow: "hidden",
    opacity: panelIn,
    transform: `translateY(${lift}px)`,
  };

  return (
    <AbsoluteFill style={{ backgroundColor: C.bg }}>
      {/* Agent session */}
      <div
        style={{
          ...pane,
          left: MX,
          width: LEFT_W,
          backgroundColor: TUI.bg,
          padding: "30px 28px",
          display: "flex",
          flexDirection: "column",
          gap: 16,
        }}
      >
        <SessionChip at={2}>agent session &middot; thursday, 9:02 am</SessionChip>
        <ToolLine at={T0} size={22}>
          lific_get_plan [plan=APP-PLAN-2]
        </ToolLine>
        <Typed at={R1} text="APP-43 is next on the plan. Starting it." />
        <ToolLine at={T1} size={22}>
          lific_update_issue [APP-43, status=active]
        </ToolLine>
      </div>

      {/* The board, live in the web UI */}
      <div
        style={{
          ...pane,
          left: MX + LEFT_W + GAP,
          width: RIGHT_W,
          backgroundColor: C.bg,
        }}
      >
        <div
          style={{
            position: "absolute",
            left: 0,
            top: 0,
            width: BOARD_W,
            height: BOARD_H,
            transform: `scale(${S})`,
            transformOrigin: "top left",
            fontFamily: BODY,
          }}
        >
          {/* Column tracks */}
          <div style={{ position: "absolute", inset: 0, display: "flex" }}>
            {(["todo", "active", "done"] as const).map((s, i) => (
              <div
                key={s}
                style={{
                  width: COL,
                  flexShrink: 0,
                  borderRight: i < 2 ? `1px solid ${C.border}` : "none",
                  boxSizing: "border-box",
                }}
              >
                <ColumnHeader status={s} count={counts[s]} />
              </div>
            ))}
          </div>

          {/* Drop-target outline */}
          {showTarget ? (
            <div
              style={{
                position: "absolute",
                left: colX(2) + 4,
                top: HEADER_H + 4,
                width: COL - 9,
                bottom: 8,
                outline: `2px dashed ${C.accent}`,
                outlineOffset: -4,
                borderRadius: 8,
              }}
            />
          ) : null}

          {/* Static cards */}
          {[
            { issue: APP48, x: colX(1) + CARD_PAD, y: slotY(0) },
            { issue: APP42, x: colX(2) + CARD_PAD, y: slotY(0) },
            { issue: APP52, x: colX(0) + CARD_PAD, y: y52 },
          ].map((c) => (
            <div key={c.issue.identifier} style={{ position: "absolute", left: c.x, top: c.y }}>
              <IssueCard issue={c.issue} width={CARD_W} />
            </div>
          ))}

          {/* APP-43: moved by the agent, arrives over the socket */}
          <div style={{ position: "absolute", left: pos43.x, top: pos43.y, zIndex: 20 }}>
            <IssueCard
              issue={{
                ...APP43,
                status: frame >= MOVE ? "active" : "todo",
                updated: frame >= MOVE ? "just now" : APP43.updated,
              }}
              width={CARD_W}
              style={
                hl43 > 0
                  ? {
                      border: `1px solid ${C.accent}`,
                      boxShadow: `0 0 ${14 * hl43}px ${C.accent}88`,
                    }
                  : undefined
              }
            />
            <Tag agent opacity={agentTag} style={{ right: -4, top: -7 }}>
              opencode-blake
            </Tag>
          </div>

          {/* APP-51: dragged by blake */}
          <div
            style={{
              position: "absolute",
              left: pos51.x,
              top: pos51.y,
              zIndex: 30,
              transform: dragging
                ? "rotate(2deg) scale(1.02)"
                : frame >= DROP
                  ? `scale(${1 + (1 - settle) * 0.03})`
                  : undefined,
              filter: dragging
                ? "drop-shadow(0 14px 22px rgba(0,0,0,0.5))"
                : doneGlow > 0
                  ? `drop-shadow(0 0 ${14 * doneGlow}px ${C.success}88)`
                  : undefined,
            }}
          >
            <IssueCard
              issue={{
                ...APP51,
                status: frame >= DROP ? "done" : "todo",
                updated: frame >= DROP ? "just now" : APP51.updated,
              }}
              width={CARD_W}
            />
          </div>

          <Cursor points={CURSOR} />
          <Tag opacity={blakeTag} style={{ left: cur.x + 14, top: cur.y + 36 }}>
            blake
          </Tag>
        </div>
      </div>

      {/* Project activity feed */}
      <div
        style={{
          position: "absolute",
          left: MX,
          top: STRIP_TOP,
          width: FEED_W,
          transform: `scale(${S}) translateY(${lift / S}px)`,
          transformOrigin: "top left",
          opacity: panelIn,
          fontFamily: BODY,
        }}
      >
        <div
          style={{
            borderRadius: 11,
            border: `1px solid ${C.border}`,
            backgroundColor: C.bgSubtle,
            padding: "8px 8px 6px",
          }}
        >
          <div
            style={{
              display: "flex",
              alignItems: "center",
              gap: 8,
              padding: "0 10px 6px",
              borderBottom: `1px solid ${C.border}`,
            }}
          >
            <History size={13} color={C.textFaint} />
            <span
              style={{
                fontSize: 11,
                fontWeight: 600,
                textTransform: "uppercase",
                letterSpacing: "0.1em",
                color: C.textMuted,
              }}
            >
              Activity
            </span>
            <span style={{ fontSize: 11, color: C.textFaint, fontVariantNumeric: "tabular-nums" }}>
              {feedCount}
            </span>
          </div>
          <div style={{ position: "relative", height: ROW_H * 2, marginTop: 4 }}>
            {frame >= FEED_O ? (
              <FeedRow
                letter="OB"
                actor="opencode-blake"
                bot
                issue="APP-43"
                from="todo"
                to="active"
                transport="mcp"
                y={(1 - inO) * -6}
                opacity={inO}
              />
            ) : null}
            {frame >= FEED_B ? (
              <FeedRow
                letter="B"
                actor="blake"
                issue="APP-51"
                from="todo"
                to="done"
                transport="web"
                y={rowB + (1 - inB) * -6}
                opacity={inB}
              />
            ) : null}
          </div>
        </div>
      </div>

      {/* Loop-seam fade */}
      <AbsoluteFill style={{ backgroundColor: C.bg, opacity: fade, pointerEvents: "none" }} />
    </AbsoluteFill>
  );
};

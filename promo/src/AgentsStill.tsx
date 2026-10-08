import React from "react";
import { AbsoluteFill, useCurrentFrame, useVideoConfig, spring } from "remotion";
import { C } from "./theme";
import { TUI, ToolLine, Typed, SessionChip } from "./PlanSync";

/*
 * AgentsStill: the landing page's "For agents" window. A fresh OpenCode
 * session on Thursday morning makes one get_briefing call and knows what
 * happened since its last session, what the plan wants next, and what
 * waits on a person. It runs a minute before the For everyone scene,
 * where the same agent moves APP-43 to Active.
 *
 * The reply is written from what get_briefing returned for a seeded
 * database holding this story (APP-PLAN-2 at 5/7, APP-42 closed and
 * APP-48 moved since the cursor, APP-44 waiting on @blake).
 *
 * The landing page uses a still with no pane chrome (the site's Window
 * draws it):
 *   bunx remotion still AgentsStill /tmp/opencode/agents-still.png --frame=150 --props='{"freeze":true}'
 *   magick /tmp/opencode/agents-still.png -crop 1400x270+36+30 +repage -quality 90 ../site/public/agents-session.webp
 */

const MX = 36;
const TOP = 30;
const PANE_W = 1400;
const PANE_H = 270;

export const AGENTS_STILL_W = PANE_W + MX * 2;
export const AGENTS_STILL_H = PANE_H + TOP * 2;
export const AGENTS_STILL_FRAMES = 180; // 6s @ 30fps

// ── Beat table (30fps) ───────────────────────────────────────
const T0 = 8; // get_briefing tool line
const R1 = 28; // reply, one line after another
const R2 = 64;
const R3 = 112;

const REPLY = [
  { at: R1, text: "Since the last session, APP-42 closed and APP-48 went active." },
  { at: R2, text: "Ship offline sync is 5/7 done. Step 4 is APP-43; APP-44 waits on you." },
  { at: R3, text: "Starting APP-43." },
];

export const AgentsStill: React.FC<{ freeze?: boolean }> = ({ freeze = false }) => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();
  const panelIn = spring({ frame, fps, config: { damping: 200, stiffness: 90 } });

  return (
    <AbsoluteFill style={{ backgroundColor: C.bg }}>
      <div
        style={{
          position: "absolute",
          left: MX,
          top: TOP,
          width: PANE_W,
          height: PANE_H,
          boxSizing: "border-box",
          borderRadius: freeze ? 0 : 16,
          border: freeze ? "none" : `1px solid ${C.border}`,
          boxShadow: freeze ? "none" : "0 30px 80px rgba(0,0,0,0.55)",
          overflow: "hidden",
          backgroundColor: TUI.bg,
          padding: "30px 28px",
          display: "flex",
          flexDirection: "column",
          gap: 16,
          opacity: panelIn,
          transform: `translateY(${(1 - panelIn) * 20}px)`,
        }}
      >
        <SessionChip at={2}>agent session &middot; thursday, 9:01 am</SessionChip>
        <ToolLine at={T0} size={22}>
          lific_get_briefing [project=APP, since=2026-10-07T06:52:00Z]
        </ToolLine>
        <div style={{ display: "flex", flexDirection: "column", gap: 6 }}>
          {REPLY.map((line) => (
            <Typed key={line.at} at={line.at} text={line.text} />
          ))}
        </div>
      </div>
    </AbsoluteFill>
  );
};

import React from "react";
import {
  AbsoluteFill,
  useCurrentFrame,
  useVideoConfig,
  spring,
  interpolate,
} from "remotion";
import { C } from "../theme";
import { BODY, DISPLAY, MONO } from "../fonts";
import { Background } from "../components/Background";
import { IssueCard } from "../components/board";
import { FadeUp } from "../components/text";

/*
 * The differentiator: an AI coding agent drives the tracker over MCP,
 * and the board reacts live.
 */

const ToolChip: React.FC<{ label: string; at: number; ok?: boolean }> = ({
  label,
  at,
  ok = true,
}) => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();
  const s = spring({ frame: frame - at, fps, config: { damping: 200, stiffness: 140 } });
  const okIn = interpolate(frame, [at + 18, at + 26], [0, 1], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
  });
  if (frame < at) return null;
  return (
    <div
      style={{
        display: "flex",
        alignItems: "center",
        gap: 10,
        fontFamily: MONO,
        fontSize: 19,
        color: C.textMuted,
        backgroundColor: C.bgSubtle,
        border: `1px solid ${C.border}`,
        borderRadius: 9,
        padding: "10px 16px",
        opacity: s,
        transform: `translateY(${(1 - s) * 14}px)`,
      }}
    >
      <span style={{ color: C.accent }}>⚙</span>
      {label}
      {ok ? (
        <span style={{ color: C.success, opacity: okIn, fontWeight: 600 }}>✓</span>
      ) : null}
    </div>
  );
};

export const AgentScene: React.FC = () => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();

  const userIn = spring({ frame: frame - 8, fps, config: { damping: 200, stiffness: 120 } });

  // Board reactions
  const doneFlash = Math.max(0, Math.min(1, (frame - 92) / 10)) * Math.max(0, 1 - (frame - 102) / 40);
  const doneCardIn = spring({ frame: frame - 88, fps, config: { damping: 16, stiffness: 130 } });
  const newCardIn = spring({ frame: frame - 152, fps, config: { damping: 15, stiffness: 120 } });

  const captionIn = interpolate(frame, [185, 205], [0, 1], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
  });

  return (
    <Background>
      <AbsoluteFill
        style={{
          flexDirection: "row",
          justifyContent: "center",
          alignItems: "center",
          gap: 46,
          paddingBottom: 60,
        }}
      >
        {/* Agent chat panel */}
        <div
          style={{
            width: 700,
            height: 620,
            borderRadius: 16,
            border: `1px solid ${C.border}`,
            backgroundColor: C.chrome,
            boxShadow: "0 30px 80px rgba(0,0,0,0.55)",
            padding: "26px 30px",
            display: "flex",
            flexDirection: "column",
            gap: 18,
          }}
        >
          <div
            style={{
              fontFamily: DISPLAY,
              fontSize: 19,
              fontWeight: 600,
              color: C.textMuted,
              paddingBottom: 14,
              borderBottom: `1px solid ${C.border}`,
            }}
          >
            coding agent
          </div>

          {/* user message */}
          <div
            style={{
              alignSelf: "flex-end",
              maxWidth: 520,
              fontFamily: BODY,
              fontSize: 22,
              color: C.stone950,
              backgroundColor: C.accent,
              borderRadius: "16px 16px 4px 16px",
              padding: "14px 20px",
              opacity: userIn,
              transform: `translateY(${(1 - userIn) * 16}px)`,
            }}
          >
            Close out the WAL race fix and file a follow-up for login
            rate-limiting.
          </div>

          <div style={{ display: "flex", flexDirection: "column", gap: 12, marginTop: 8 }}>
            <ToolChip at={70} label="lific · update_issue LIF-198 → done" />
            <ToolChip at={130} label='lific · create_issue "Rate-limit login endpoint"' />
          </div>

          <FadeUp delay={175} style={{ marginTop: 4 }}>
            <div
              style={{
                fontFamily: BODY,
                fontSize: 21,
                color: C.text,
                lineHeight: 1.5,
              }}
            >
              Done — LIF-198 closed, follow-up filed as{" "}
              <span style={{ fontFamily: MONO, color: C.accent }}>LIF-232</span>.
            </div>
          </FadeUp>
        </div>

        {/* Live board reaction */}
        <div
          style={{
            width: 700,
            height: 620,
            borderRadius: 16,
            border: `1px solid ${C.border}`,
            backgroundColor: C.bg,
            boxShadow: "0 30px 80px rgba(0,0,0,0.55)",
            padding: "26px 30px",
            display: "flex",
            gap: 26,
          }}
        >
          {/* Todo column */}
          <div style={{ flex: 1, display: "flex", flexDirection: "column", gap: 12 }}>
            <ColHeader dot={C.textFaint} name="Todo" />
            <IssueCard
              card={{ id: "LIF-226", title: "MCP: recurring plan templates", priority: "medium", module: "MCP" }}
              width={300}
            />
            {frame >= 152 ? (
              <div
                style={{
                  opacity: newCardIn,
                  transform: `scale(${0.9 + newCardIn * 0.1}) translateY(${(1 - newCardIn) * -18}px)`,
                }}
              >
                <IssueCard
                  card={{ id: "LIF-232", title: "Rate-limit login endpoint", priority: "high", module: "Auth" }}
                  width={300}
                  highlight={Math.max(0, 1 - (frame - 165) / 45)}
                />
              </div>
            ) : null}
          </div>

          {/* Done column */}
          <div style={{ flex: 1, display: "flex", flexDirection: "column", gap: 12 }}>
            <ColHeader dot={C.success} name="Done" />
            {frame >= 88 ? (
              <div
                style={{
                  opacity: doneCardIn,
                  transform: `scale(${0.92 + doneCardIn * 0.08})`,
                }}
              >
                <IssueCard
                  card={{ id: "LIF-198", title: "Fix WAL checkpoint race on shutdown", priority: "high", module: "Core" }}
                  width={300}
                  highlight={doneFlash}
                />
              </div>
            ) : null}
            <IssueCard
              card={{ id: "LIF-183", title: "OAuth device flow for CLI login", module: "Auth" }}
              width={300}
            />
          </div>
        </div>
      </AbsoluteFill>

      <div
        style={{
          position: "absolute",
          bottom: 54,
          width: "100%",
          textAlign: "center",
          fontFamily: BODY,
          fontSize: 40,
          fontWeight: 500,
          color: C.text,
          opacity: captionIn,
        }}
      >
        Your coding agents are first-class citizens.{" "}
        <span style={{ color: C.accent, fontWeight: 600 }}>MCP built in.</span>
      </div>
    </Background>
  );
};

const ColHeader: React.FC<{ dot: string; name: string }> = ({ dot, name }) => (
  <div style={{ display: "flex", alignItems: "center", gap: 9 }}>
    <span style={{ width: 10, height: 10, borderRadius: 5, backgroundColor: dot }} />
    <span style={{ fontFamily: DISPLAY, fontSize: 17, fontWeight: 600, color: C.text }}>
      {name}
    </span>
  </div>
);

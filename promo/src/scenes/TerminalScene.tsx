import React from "react";
import { AbsoluteFill, useCurrentFrame, interpolate } from "remotion";
import { C } from "../theme";
import { BODY, MONO } from "../fonts";
import { Background } from "../components/Background";
import { Terminal, TermLine } from "../components/Terminal";

// Real CLI output: default port 8080, real tracing lines from src/main.rs.
const LINES: TermLine[] = [
  { at: 10, text: "cargo install lific", kind: "cmd", fpc: 1.6 },
  { at: 52, text: "    Updating crates.io index", kind: "out" },
  { at: 62, text: "   Compiling lific v2.0.0", kind: "out" },
  { at: 84, text: "    Finished `release` profile [optimized]", kind: "out" },
  { at: 92, text: "   Installed package `lific v2.0.0` (executable `lific`)", kind: "ok" },
  { at: 116, text: "lific start", kind: "cmd", fpc: 1.8 },
  { at: 148, text: "INFO database ready path=lific.db", kind: "out" },
  { at: 158, text: "INFO API key auth enabled active_keys=1", kind: "out" },
  {
    at: 170,
    text: "INFO lific server started (REST + MCP + OAuth at /mcp) addr=0.0.0.0:8080",
    kind: "info",
  },
];

/** `lific start` is submitted here; the server line lands at 170. */
const START_AT = 116;
const DEPLOYED_AT = 170;

export const TerminalScene: React.FC = () => {
  const frame = useCurrentFrame();

  // Honest stopwatch: real wall-clock seconds from `lific start` to the
  // server-started line. No dramatization — the startup really is ~2s.
  const elapsed = Math.max(0, Math.min(frame, DEPLOYED_AT) - START_AT) / 30;
  const clock = `${elapsed.toFixed(1)}s`;
  const deployed = frame >= DEPLOYED_AT;

  const captionIn = interpolate(frame, [195, 215], [0, 1], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
  });

  return (
    <Background>
      <AbsoluteFill style={{ justifyContent: "center", alignItems: "center" }}>
        <Terminal lines={LINES} />

        {/* Deploy stopwatch — appears when `lific start` is typed */}
        <div
          style={{
            position: "absolute",
            top: 70,
            right: 110,
            opacity: interpolate(frame, [START_AT - 8, START_AT], [0, 1], {
              extrapolateLeft: "clamp",
              extrapolateRight: "clamp",
            }),
            fontFamily: MONO,
            fontSize: 44,
            fontWeight: 600,
            color: deployed ? C.success : C.textMuted,
            border: `1px solid ${deployed ? C.success : C.border}`,
            backgroundColor: C.bgSubtle,
            borderRadius: 14,
            padding: "12px 26px",
            display: "flex",
            alignItems: "center",
            gap: 16,
          }}
        >
          <span>{clock}</span>
          {deployed ? <span style={{ fontSize: 30 }}>deployed</span> : null}
        </div>

        <div
          style={{
            position: "absolute",
            bottom: 90,
            fontFamily: BODY,
            fontSize: 40,
            fontWeight: 500,
            color: C.text,
            opacity: captionIn,
          }}
        >
          Deploys in{" "}
          <span style={{ color: C.success, fontWeight: 600 }}>seconds.</span>{" "}
          <span style={{ color: C.textMuted }}>
            REST, MCP, web UI — one process. No containers.
          </span>
        </div>
      </AbsoluteFill>
    </Background>
  );
};

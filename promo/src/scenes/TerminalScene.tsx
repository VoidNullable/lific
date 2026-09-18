import React from "react";
import {
  AbsoluteFill,
  useCurrentFrame,
  useVideoConfig,
  spring,
  interpolate,
} from "remotion";
import { C } from "../theme";
import { BODY, MONO } from "../fonts";
import { Background } from "../components/Background";
import { Terminal, TermLine } from "../components/Terminal";

/*
 * Two real shells, like real life: `lific start` holds window 1 in the
 * foreground; window 2 opens on top to run `lific connect`. Every line
 * mirrors actual CLI output (src/main.rs tracing, src/cli/connect
 * outcome format, default port 3456 from src/config.rs).
 */

const WIN1_LINES: TermLine[] = [
  { at: 6, text: "cargo install lific", kind: "cmd", fpc: 1.1 },
  { at: 34, text: "    Updating crates.io index", kind: "out" },
  { at: 42, text: "   Compiling lific v2.0.0", kind: "out" },
  { at: 56, text: "    Finished `release` profile [optimized]", kind: "out" },
  {
    at: 63,
    text: "   Installed package `lific v2.0.0` (executable `lific`)",
    kind: "ok",
  },
  { at: 78, text: "lific start", kind: "cmd", fpc: 1.2 },
  { at: 98, text: "INFO database ready path=lific.db", kind: "out" },
  { at: 106, text: "INFO API key auth enabled active_keys=1", kind: "out" },
  {
    at: 117,
    text: "INFO lific server started (REST + MCP + OAuth at /mcp) addr=0.0.0.0:3456",
    kind: "info",
  },
];

/** `lific start` is submitted here; the server line lands on bar 13
 *  of the 130 BPM grid (global frame 665). */
const START_AT = 78;
const DEPLOYED_AT = 117;

/** Window 2 opens on beat 50 of the grid (global 692, scene-local 144):
 *  the server owns window 1's foreground, so connect runs in a fresh
 *  shell — exactly like real usage. */
const WIN2_AT = 144;

// Output format is verbatim from src/cli/connect/mod.rs:
// "  [{display}] {action}: {path}" + restart hint.
const WIN2_LINES: TermLine[] = [
  {
    at: WIN2_AT + 8,
    text: "lific connect --client opencode --yes",
    kind: "cmd",
    fpc: 0.9,
  },
  {
    at: WIN2_AT + 50,
    text: "  [OpenCode] created: /home/lizzy/.config/opencode/opencode.json",
    kind: "ok",
  },
  { at: WIN2_AT + 60, text: " ", kind: "out" },
  {
    at: WIN2_AT + 62,
    text: "  Restart your client(s) to pick up the new MCP server.",
    kind: "out",
  },
];

export const TerminalScene: React.FC = () => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();

  // Honest stopwatch: real wall-clock seconds from `lific start` to the
  // server-started line. No dramatization — the startup really is ~2s.
  const elapsed = Math.max(0, Math.min(frame, DEPLOYED_AT) - START_AT) / 30;
  const clock = `${elapsed.toFixed(1)}s`;
  const deployed = frame >= DEPLOYED_AT;

  const captionIn = interpolate(frame, [66, 82], [0, 1], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
  });

  // Window 2 springs in over window 1; window 1 recedes slightly.
  const win2In = spring({
    frame: frame - WIN2_AT,
    fps,
    config: { damping: 18, stiffness: 130, mass: 0.8 },
  });
  const win1Dim = interpolate(frame, [WIN2_AT, WIN2_AT + 14], [1, 0.5], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
  });

  return (
    <Background>
      <AbsoluteFill style={{ justifyContent: "center", alignItems: "center" }}>
        <div style={{ position: "relative", marginTop: -34 }}>
          {/* Window 1: install + start (server stays in the foreground) */}
          <div
            style={{
              opacity: win1Dim,
              transform: `translate(${win2In * -36}px, ${win2In * -26}px) scale(${1 - win2In * 0.03})`,
            }}
          >
            <Terminal
              lines={WIN1_LINES}
              width={1460}
              height={620}
              fontSize={30}
            />
          </div>

          {/* Window 2: a fresh shell for `lific connect` */}
          {frame >= WIN2_AT ? (
            <div
              style={{
                position: "absolute",
                left: 90,
                top: 210,
                opacity: Math.min(1, win2In * 1.6),
                transform: `translateY(${(1 - win2In) * 60}px) scale(${0.96 + win2In * 0.04})`,
                filter: "drop-shadow(0 30px 70px rgba(0,0,0,0.65))",
              }}
            >
              <Terminal
                lines={WIN2_LINES}
                width={1380}
                height={330}
                fontSize={30}
                title="fish — ~ (2)"
              />
            </div>
          ) : null}
        </div>

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
            fontSize: 50,
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
            bottom: 56,
            fontFamily: BODY,
            fontSize: 44,
            fontWeight: 500,
            color: C.text,
            opacity: captionIn,
            textShadow: "0 4px 30px rgba(0,0,0,0.9)",
          }}
        >
          Deploys in{" "}
          <span style={{ color: C.success, fontWeight: 600 }}>seconds.</span>{" "}
          <span style={{ color: C.textMuted }}>
            One more command connects your agents.
          </span>
        </div>
      </AbsoluteFill>
    </Background>
  );
};

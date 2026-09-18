import React from "react";
import { AbsoluteFill, useCurrentFrame, interpolate } from "remotion";
import { C } from "../theme";
import { BODY, MONO } from "../fonts";
import { Background } from "../components/Background";

/*
 * Ad B terminal beat: the 2.0 `lific init` clack-style session. One
 * command builds a running, boot-persistent instance (real behavior:
 * writes config, migrates db, mints key, installs a systemd user unit,
 * verifies the server answers). Condensed for feed pacing.
 */

type Line = { at: number; text: string; color?: string; typed?: boolean };

const LINES: Line[] = [
  { at: 8, text: "lific init", typed: true },
  { at: 34, text: "┌  lific init", color: C.textMuted },
  { at: 46, text: "◇  Wrote lific.toml" },
  { at: 56, text: "◇  Database created and migrated" },
  { at: 66, text: "◇  API key minted (shown once)" },
  { at: 78, text: "◇  Service installed. Starts on boot." },
  { at: 94, text: "└  Running at http://localhost:3456", color: C.success },
];

export const InitScene: React.FC = () => {
  const frame = useCurrentFrame();
  const capIn = interpolate(frame, [40, 54], [0, 1], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
  });

  return (
    <Background>
      <AbsoluteFill style={{ justifyContent: "center", alignItems: "center" }}>
        <div
          style={{
            width: 1300,
            height: 520,
            borderRadius: 14,
            border: `1px solid ${C.border}`,
            backgroundColor: C.chrome,
            boxShadow: "0 30px 80px rgba(0,0,0,0.55)",
            overflow: "hidden",
            marginTop: -60,
          }}
        >
          <div
            style={{
              height: 46,
              display: "flex",
              alignItems: "center",
              gap: 8,
              padding: "0 18px",
              borderBottom: `1px solid ${C.border}`,
              backgroundColor: C.bgSubtle,
            }}
          >
            {["#f87171", "#fbbf24", "#4ade80"].map((c) => (
              <div
                key={c}
                style={{
                  width: 13,
                  height: 13,
                  borderRadius: 7,
                  backgroundColor: c,
                }}
              />
            ))}
            <div
              style={{
                flex: 1,
                textAlign: "center",
                fontFamily: MONO,
                fontSize: 15,
                color: C.textFaint,
              }}
            >
              fish — ~
            </div>
            <div style={{ width: 55 }} />
          </div>
          <div
            style={{
              padding: "24px 30px",
              fontFamily: MONO,
              fontSize: 31,
              lineHeight: 1.7,
            }}
          >
            {LINES.filter((l) => frame >= l.at).map((l, i) => {
              const chars = l.typed
                ? Math.min(l.text.length, Math.floor((frame - l.at) / 1.2))
                : l.text.length;
              return (
                <div
                  key={i}
                  style={{ color: l.color ?? C.text, whiteSpace: "pre" }}
                >
                  {l.typed ? (
                    <span style={{ color: C.success }}>{"$ "}</span>
                  ) : null}
                  {l.text.slice(0, chars)}
                </div>
              );
            })}
          </div>
        </div>

        <div
          style={{
            position: "absolute",
            bottom: 64,
            fontFamily: BODY,
            fontSize: 44,
            fontWeight: 500,
            color: C.text,
            opacity: capIn,
            textShadow: "0 4px 30px rgba(0,0,0,0.9)",
          }}
        >
          One command.{" "}
          <span style={{ color: C.success, fontWeight: 600 }}>
            Running, and it survives reboots.
          </span>
        </div>
      </AbsoluteFill>
    </Background>
  );
};

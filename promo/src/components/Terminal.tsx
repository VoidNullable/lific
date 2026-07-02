import React from "react";
import { useCurrentFrame, interpolate } from "remotion";
import { C } from "../theme";
import { MONO } from "../fonts";

export type TermLine = {
  /** Frame (relative to the terminal's sequence) when this line starts. */
  at: number;
  text: string;
  /** "cmd" lines get a $ prompt and type out; others print instantly. */
  kind?: "cmd" | "out" | "ok" | "info";
  /** Frames per character for typed lines. */
  fpc?: number;
};

const lineColor = (kind: TermLine["kind"]): string => {
  switch (kind) {
    case "cmd":
      return C.text;
    case "ok":
      return C.success;
    case "info":
      return C.accent;
    default:
      return C.textMuted;
  }
};

/**
 * Typewriter terminal. Reveal is done by string slicing (never per-char
 * opacity) so the layout and cursor position stay honest.
 */
export const Terminal: React.FC<{
  lines: TermLine[];
  width?: number;
  height?: number;
  fontSize?: number;
  title?: string;
}> = ({ lines, width = 1240, height = 640, fontSize = 26, title = "fish — ~" }) => {
  const frame = useCurrentFrame();

  const visible = lines.filter((l) => frame >= l.at);
  const active = visible[visible.length - 1];

  // Smooth cursor blink (3-keyframe, not a binary toggle).
  const blink = interpolate(frame % 24, [0, 12, 24], [1, 0.15, 1]);

  return (
    <div
      style={{
        width,
        height,
        borderRadius: 14,
        border: `1px solid ${C.border}`,
        backgroundColor: C.chrome,
        boxShadow: "0 30px 80px rgba(0,0,0,0.55)",
        overflow: "hidden",
        display: "flex",
        flexDirection: "column",
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
            style={{ width: 13, height: 13, borderRadius: 7, backgroundColor: c }}
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
          {title}
        </div>
        <div style={{ width: 55 }} />
      </div>
      <div
        style={{
          flex: 1,
          padding: "22px 28px",
          fontFamily: MONO,
          fontSize,
          lineHeight: 1.65,
          whiteSpace: "pre-wrap",
        }}
      >
        {visible.map((l, i) => {
          const isTyped = l.kind === "cmd";
          const fpc = l.fpc ?? 1.4;
          const chars = isTyped
            ? Math.min(l.text.length, Math.floor((frame - l.at) / fpc))
            : l.text.length;
          const done = chars >= l.text.length;
          const isActive = l === active;
          return (
            <div key={i} style={{ color: lineColor(l.kind) }}>
              {isTyped ? (
                <span style={{ color: C.success }}>{"$ "}</span>
              ) : null}
              {l.text.slice(0, chars)}
              {isActive && (!done || isTyped) ? (
                <span
                  style={{
                    display: "inline-block",
                    width: fontSize * 0.55,
                    height: fontSize * 1.05,
                    marginLeft: 2,
                    verticalAlign: "text-bottom",
                    backgroundColor: C.text,
                    opacity: blink,
                  }}
                />
              ) : null}
            </div>
          );
        })}
      </div>
    </div>
  );
};

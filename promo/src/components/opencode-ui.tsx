import React from "react";
import { useCurrentFrame, interpolate } from "remotion";
import { MONO } from "../fonts";

/*
 * Pixel-faithful replica of the OpenCode TUI (opencode.ai / github.com/
 * anomalyco/opencode), dark theme. Everything is monospace on a very dark
 * blue-black terminal floor (#0a0e14). Blocks (user message + input) sit on
 * a darker near-black bg (#050608) with a 4px purple accent bar on the far
 * left edge. Timings/labels are deliberately model-agnostic — the string
 * "Agent" replaces any real model/vendor name.
 *
 * All motion is a pure function of useCurrentFrame(): cursor blink is a
 * 3-keyframe interpolate, tool lines reveal by string slicing.
 */

// ── TUI palette (authoritative from the spec) ───────────────
const TUI = {
  bg: "#0a0e14", // main terminal floor
  block: "#050608", // user message / input block bg (darker)
  purple: "#9d7cd8", // opencode primary
  gold: "#d0a04f", // amber/gold
  text: "#e8eaf0", // bright gray-white
  dim: "#707886", // dim gray thinking/tool text
  cursor: "#c8cdd8", // block cursor
} as const;

// Type scale
const FS = 21; // primary mono
const FS_SM = 19; // thinking / tool / status lines
const FS_STATUS = 16; // bottom status bar

const PAD_X = 40; // block horizontal padding / text left rail
const BAR_W = 4; // purple accent bar width

/** A full-width dark block with the purple left accent bar. */
const AccentBlock: React.FC<{
  children?: React.ReactNode;
  style?: React.CSSProperties;
}> = ({ children, style }) => (
  <div
    style={{
      position: "relative",
      backgroundColor: TUI.block,
      boxSizing: "border-box",
      ...style,
    }}
  >
    <div
      style={{
        position: "absolute",
        left: 0,
        top: 0,
        bottom: 0,
        width: BAR_W,
        backgroundColor: TUI.purple,
      }}
    />
    {children}
  </div>
);

/** Reveal a string by slicing on frame; `at` is when typing starts. */
const sliceReveal = (text: string, frame: number, at: number, fpc = 1.0) => {
  if (frame < at) return "";
  return text.slice(0, Math.floor((frame - at) / fpc));
};

export type OpenCodeProps = {
  width?: number;
  height?: number;
  userText: string;
  userFrom: number;
  thought: string; // e.g. "Thought: 312ms"
  thinking: string; // dim thinking line
  thinkAt: number;
  tool1: string;
  tool1At: number;
  tool2: string;
  tool2At: number;
  reply: string;
  replyAt: number;
  completeAt: number;
  completeText: string; // e.g. "Build · Agent · 4.1s" (▣ prepended)
};

export const OpenCodeTUI: React.FC<OpenCodeProps> = ({
  width = 880,
  height = 750,
  userText,
  userFrom,
  thought,
  thinking,
  thinkAt,
  tool1,
  tool1At,
  tool2,
  tool2At,
  reply,
  replyAt,
  completeAt,
  completeText,
}) => {
  const frame = useCurrentFrame();

  // User block quick fade/rise.
  const userIn = interpolate(frame, [userFrom, userFrom + 8], [0, 1], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
  });

  // Thought + thinking fade.
  const thinkIn = interpolate(frame, [thinkAt, thinkAt + 8], [0, 1], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
  });

  // Tool calls POP in (quick fade, full text at once) — like real tool
  // results appearing; the streamed text is the reply, not the tools.
  const tool1In = interpolate(frame, [tool1At, tool1At + 6], [0, 1], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
  });
  const tool2In = interpolate(frame, [tool2At, tool2At + 6], [0, 1], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
  });

  // Reply TYPES out via string slicing (streaming), ~1.7 chars/frame.
  const replyText = sliceReveal(reply, frame, replyAt, 0.6);
  const replyTyping = frame >= replyAt && replyText.length < reply.length;

  // Completion line fade.
  const completeIn = interpolate(frame, [completeAt, completeAt + 8], [0, 1], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
  });

  // Block cursor blink — pure function of frame.
  const blink = interpolate(frame % 24, [0, 12, 24], [1, 0.15, 1]);

  return (
    <div
      style={{
        width,
        height,
        borderRadius: 16,
        border: "1px solid #3d4842",
        boxShadow: "0 30px 80px rgba(0,0,0,0.55)",
        overflow: "hidden",
        backgroundColor: TUI.bg,
        boxSizing: "border-box",
        display: "flex",
        flexDirection: "column",
        fontFamily: MONO,
        position: "relative",
      }}
    >
      {/* Scrollback area (grows), input pinned at bottom. */}
      <div
        style={{
          flex: 1,
          minHeight: 0,
          display: "flex",
          flexDirection: "column",
          overflow: "hidden",
        }}
      >
        {/* 1. User message block */}
        <AccentBlock
          style={{
            width: "100%",
            padding: `28px ${PAD_X}px`,
            opacity: userIn,
            transform: `translateY(${(1 - userIn) * 10}px)`,
          }}
        >
          <div
            style={{
              fontFamily: MONO,
              fontSize: FS,
              lineHeight: 1.5,
              color: TUI.text,
              paddingLeft: 2,
            }}
          >
            {userText}
          </div>
        </AccentBlock>

        {/* 2. Assistant turn on plain bg, left-aligned to block text rail */}
        <div
          style={{
            padding: `22px ${PAD_X}px`,
            display: "flex",
            flexDirection: "column",
            fontFamily: MONO,
          }}
        >
          {/* Thought: Xms (gold) */}
          <div
            style={{
              fontSize: FS_SM,
              color: TUI.gold,
              opacity: thinkIn,
            }}
          >
            {thought}
          </div>

          {/* blank line */}
          <div style={{ height: FS_SM }} />

          {/* thinking line (dim) */}
          <div
            style={{
              fontSize: FS_SM,
              lineHeight: 1.5,
              color: TUI.dim,
              opacity: thinkIn,
            }}
          >
            {thinking}
          </div>

          {/* blank line */}
          <div style={{ height: FS_SM }} />

          {/* tool line 1 — pops in whole */}
          <div
            style={{
              fontSize: FS_SM,
              lineHeight: 1.6,
              color: TUI.dim,
              minHeight: FS_SM * 1.6,
              whiteSpace: "pre",
              opacity: tool1In,
            }}
          >
            {frame >= tool1At ? (
              <>
                <span>⚙ </span>
                {tool1}
              </>
            ) : null}
          </div>

          {/* tool line 2 — pops in whole */}
          <div
            style={{
              fontSize: FS_SM,
              lineHeight: 1.6,
              color: TUI.dim,
              minHeight: FS_SM * 1.6,
              whiteSpace: "pre",
              opacity: tool2In,
            }}
          >
            {frame >= tool2At ? (
              <>
                <span>⚙ </span>
                {tool2}
              </>
            ) : null}
          </div>

          {/* blank line */}
          <div style={{ height: FS }} />

          {/* assistant reply (bright) — streams in like real output */}
          <div
            style={{
              fontSize: FS,
              lineHeight: 1.5,
              color: TUI.text,
              minHeight: FS * 1.5,
            }}
          >
            {replyText}
            {replyTyping ? (
              <span
                style={{
                  display: "inline-block",
                  width: 11,
                  height: FS,
                  marginLeft: 2,
                  verticalAlign: "text-bottom",
                  backgroundColor: TUI.cursor,
                }}
              />
            ) : null}
          </div>

          {/* blank line */}
          <div style={{ height: FS }} />

          {/* completion line ▣ Build · Agent · 4.1s */}
          <div
            style={{
              fontSize: FS_SM,
              color: TUI.dim,
              opacity: completeIn,
              whiteSpace: "pre",
            }}
          >
            {frame >= completeAt ? (
              <>
                <span style={{ color: TUI.purple }}>▣ </span>
                <span style={{ color: TUI.text }}>Build</span>
                <span style={{ color: TUI.dim }}>{completeText}</span>
              </>
            ) : null}
          </div>
        </div>
      </div>

      {/* 3. Input box (pinned near bottom) */}
      <AccentBlock
        style={{
          width: "100%",
          height: 110,
          flexShrink: 0,
          padding: `20px ${PAD_X}px`,
          display: "flex",
          flexDirection: "column",
          justifyContent: "space-between",
        }}
      >
        {/* block cursor at empty prompt */}
        <div style={{ display: "flex", alignItems: "center", height: 30 }}>
          <div
            style={{
              width: 14,
              height: 28,
              backgroundColor: TUI.cursor,
              opacity: blink,
            }}
          />
        </div>
        {/* status line: Build · Agent · high */}
        <div style={{ fontSize: FS_SM, whiteSpace: "pre" }}>
          <span style={{ color: TUI.purple }}>Build</span>
          <span style={{ color: TUI.dim }}> · </span>
          <span style={{ color: TUI.dim }}>Agent</span>
          <span style={{ color: TUI.dim }}> · </span>
          <span style={{ color: TUI.gold, fontWeight: 700 }}>high</span>
        </div>
      </AccentBlock>

      {/* 4. Bottom status bar (right-aligned, small) */}
      <div
        style={{
          flexShrink: 0,
          display: "flex",
          justifyContent: "flex-end",
          alignItems: "center",
          gap: 24,
          padding: "8px 16px",
          fontSize: FS_STATUS,
          whiteSpace: "pre",
          backgroundColor: TUI.bg,
        }}
      >
        <span style={{ color: TUI.dim }}>49.9K (5%) · $0.36</span>
        <span>
          <span style={{ color: "#9aa0ac" }}>ctrl+p</span>
          <span style={{ color: TUI.dim }}> commands</span>
        </span>
      </div>
    </div>
  );
};

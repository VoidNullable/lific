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
import { Circle, CircleCheckBig } from "../components/icons";

/*
 * Ad B's new wedge scene (lands on the track's second drop): plans are
 * durable agent memory. Session 1 creates a plan; the context window
 * dies; session 2 calls get_plan and resumes exactly where it left off.
 * Vocabulary is the community's own: "survives the context window",
 * "state, not transcript".
 */

const TUI = {
  bg: "#0a0e14",
  block: "#050608",
  purple: "#9d7cd8",
  gold: "#d0a04f",
  text: "#e8eaf0",
  dim: "#707886",
} as const;

const S1_TOOL = 10;
const TREE_IN = 26;
const WIPE = 78;
const S2_LABEL = 104;
const S2_TOOL = 116;
const TREE_GLOW = 132;
const S2_REPLY = 146;
const CAP_1 = 96;
const CAP_2 = 118;

const STEPS: { title: string; done: boolean }[] = [
  { title: "Extract billing module", done: true },
  { title: "Add idempotency keys", done: true },
  { title: "Migrate webhook handlers", done: false },
  { title: "Cut over and delete legacy path", done: false },
];

const SessionChip: React.FC<{ at: number; children: React.ReactNode }> = ({
  at,
  children,
}) => {
  const frame = useCurrentFrame();
  if (frame < at) return null;
  const t = interpolate(frame, [at, at + 8], [0, 1], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
  });
  return (
    <div
      style={{
        display: "inline-flex",
        alignSelf: "flex-start",
        padding: "5px 14px",
        borderRadius: 999,
        border: `1px solid ${TUI.dim}`,
        fontFamily: MONO,
        fontSize: 17,
        color: TUI.dim,
        opacity: t,
      }}
    >
      {children}
    </div>
  );
};

const ToolLine: React.FC<{ at: number; children: React.ReactNode }> = ({
  at,
  children,
}) => {
  const frame = useCurrentFrame();
  if (frame < at) return null;
  const t = interpolate(frame, [at, at + 6], [0, 1], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
  });
  const okIn = interpolate(frame, [at + 10, at + 16], [0, 1], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
  });
  return (
    <div
      style={{
        fontFamily: MONO,
        fontSize: 21,
        color: TUI.dim,
        opacity: t,
        whiteSpace: "pre",
      }}
    >
      <span>⚙ </span>
      {children}
      <span style={{ color: C.success, opacity: okIn, fontWeight: 600 }}>
        {" "}
        ✓
      </span>
    </div>
  );
};

export const PlansScene: React.FC = () => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();

  // Session 1 content dims when the context window is wiped.
  const s1Dim = interpolate(frame, [WIPE, WIPE + 14], [1, 0.28], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
  });
  const wipeFlash = frame >= WIPE ? Math.max(0, 1 - (frame - WIPE) / 26) : 0;

  const treeIn = spring({
    frame: frame - TREE_IN,
    fps,
    config: { damping: 16, stiffness: 120 },
  });
  const glow =
    frame >= TREE_GLOW ? Math.max(0, 1 - (frame - TREE_GLOW) / 44) : 0;

  const replyText = "Resuming: step 3, migrate webhook handlers.";
  const replyChars =
    frame >= S2_REPLY
      ? Math.min(replyText.length, Math.floor((frame - S2_REPLY) * 1.4))
      : 0;

  const cap = (at: number) =>
    interpolate(frame, [at, at + 14], [0, 1], {
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
          gap: 44,
          paddingBottom: 140,
        }}
      >
        {/* Agent terminal panel */}
        <div
          style={{
            width: 880,
            height: 560,
            borderRadius: 16,
            border: `1px solid ${C.border}`,
            backgroundColor: TUI.bg,
            boxShadow: "0 30px 80px rgba(0,0,0,0.55)",
            padding: "28px 34px",
            display: "flex",
            flexDirection: "column",
            gap: 16,
            boxSizing: "border-box",
            position: "relative",
            overflow: "hidden",
          }}
        >
          <div
            style={{
              display: "flex",
              flexDirection: "column",
              gap: 14,
              opacity: s1Dim,
            }}
          >
            <SessionChip at={0}>session 1</SessionChip>
            <ToolLine at={S1_TOOL}>
              lific_create_plan [title=Payment refactor, steps=4]
            </ToolLine>
            <div
              style={{
                fontFamily: MONO,
                fontSize: 20,
                color: TUI.text,
                opacity: frame >= S1_TOOL + 18 ? 1 : 0,
              }}
            >
              Plan LIF-PLAN-7 created. Working through it.
            </div>
          </div>

          {/* context wipe divider */}
          {frame >= WIPE ? (
            <div
              style={{
                display: "flex",
                alignItems: "center",
                gap: 14,
                opacity: interpolate(frame, [WIPE, WIPE + 10], [0, 1], {
                  extrapolateLeft: "clamp",
                  extrapolateRight: "clamp",
                }),
              }}
            >
              <div
                style={{ flex: 1, height: 1, backgroundColor: `${C.error}66` }}
              />
              <span style={{ fontFamily: MONO, fontSize: 17, color: C.error }}>
                context window cleared
              </span>
              <div
                style={{ flex: 1, height: 1, backgroundColor: `${C.error}66` }}
              />
            </div>
          ) : null}

          <div style={{ display: "flex", flexDirection: "column", gap: 14 }}>
            <SessionChip at={S2_LABEL}>session 2 · fresh context</SessionChip>
            <ToolLine at={S2_TOOL}>lific_get_plan [plan=LIF-PLAN-7]</ToolLine>
            <div style={{ fontFamily: MONO, fontSize: 20, color: TUI.text }}>
              {replyText.slice(0, replyChars)}
              {replyChars > 0 && replyChars < replyText.length ? (
                <span
                  style={{
                    display: "inline-block",
                    width: 10,
                    height: 20,
                    marginLeft: 2,
                    backgroundColor: "#c8cdd8",
                    verticalAlign: "text-bottom",
                  }}
                />
              ) : null}
            </div>
          </div>

          {wipeFlash > 0 ? (
            <AbsoluteFill
              style={{
                backgroundColor: `rgba(248,113,113,${wipeFlash * 0.06})`,
              }}
            />
          ) : null}
        </div>

        {/* The plan: durable state in Lific */}
        <div
          style={{
            width: 600,
            borderRadius: 16,
            border: `1px solid ${glow > 0 ? C.accent : C.border}`,
            backgroundColor: C.bgSubtle,
            boxShadow:
              glow > 0
                ? `0 0 ${44 * glow}px ${C.accent}55, 0 30px 80px rgba(0,0,0,0.55)`
                : "0 30px 80px rgba(0,0,0,0.55)",
            padding: "28px 30px",
            opacity: treeIn,
            transform: `translateY(${(1 - treeIn) * 26}px)`,
            boxSizing: "border-box",
          }}
        >
          <div
            style={{
              display: "flex",
              alignItems: "center",
              gap: 12,
              paddingBottom: 16,
            }}
          >
            <span
              style={{
                fontFamily: MONO,
                fontSize: 14,
                color: C.accent,
                backgroundColor: C.accentSubtle,
                borderRadius: 6,
                padding: "2px 9px",
              }}
            >
              LIF-PLAN-7
            </span>
            <span
              style={{
                fontFamily: DISPLAY,
                fontSize: 26,
                fontWeight: 600,
                color: C.text,
              }}
            >
              Payment refactor
            </span>
          </div>
          <div style={{ display: "flex", flexDirection: "column", gap: 14 }}>
            {STEPS.map((step, i) => (
              <div
                key={step.title}
                style={{ display: "flex", alignItems: "center", gap: 13 }}
              >
                {step.done ? (
                  <CircleCheckBig size={20} color={C.success} />
                ) : (
                  <Circle
                    size={20}
                    color={
                      i === 2 && frame >= TREE_GLOW ? C.accent : C.textMuted
                    }
                  />
                )}
                <span
                  style={{
                    fontFamily: BODY,
                    fontSize: 21,
                    color: step.done ? C.textMuted : C.text,
                    textDecoration: step.done ? "line-through" : "none",
                  }}
                >
                  {step.title}
                </span>
                {i === 2 && frame >= TREE_GLOW ? (
                  <span
                    style={{
                      marginLeft: "auto",
                      fontFamily: MONO,
                      fontSize: 14,
                      color: C.accent,
                      opacity: cap(TREE_GLOW + 6),
                    }}
                  >
                    ← resuming
                  </span>
                ) : null}
              </div>
            ))}
          </div>
        </div>
      </AbsoluteFill>

      {/* Captions */}
      <div
        style={{
          position: "absolute",
          bottom: 96,
          width: "100%",
          textAlign: "center",
          fontFamily: BODY,
          fontSize: 48,
          fontWeight: 600,
          color: C.text,
          opacity: cap(CAP_1),
          textShadow: "0 4px 30px rgba(0,0,0,0.9)",
        }}
      >
        Plans survive the context window.
      </div>
      <div
        style={{
          position: "absolute",
          bottom: 44,
          width: "100%",
          textAlign: "center",
          fontFamily: BODY,
          fontSize: 30,
          color: C.textMuted,
          opacity: cap(CAP_2),
          textShadow: "0 4px 30px rgba(0,0,0,0.9)",
        }}
      >
        Shared state your agents resume. Not a transcript.
      </div>
    </Background>
  );
};

import React from "react";
import {
  AbsoluteFill,
  useCurrentFrame,
  useVideoConfig,
  spring,
  interpolate,
} from "remotion";
import { C } from "./theme";
import { BODY, DISPLAY, MONO } from "./fonts";
import { Circle, CircleCheckBig, CircleDot } from "./components/icons";

/*
 * PlanSync — landing-page loop for the "sprint is the plan" section.
 * Left: an agent terminal checks off a SUB-step, then its parent step
 * (which closes the mirrored issue). Right: the plan tree updates,
 * with the discrete tally ticking 3/7 -> 4/7 -> 5/7. Then a NEW
 * session calls get_plan and resumes from the next step. Tells the
 * whole story, including nesting, to someone who has never seen Lific.
 *
 * Render: bunx remotion render PlanSync ../site/public/plan-sync.mp4
 */

export const PLAN_SYNC_W = 1832;
export const PLAN_SYNC_H = 620;
export const PLAN_SYNC_FRAMES = 238;

const TUI = {
  bg: "#0a0e14",
  text: "#e8eaf0",
  dim: "#707886",
} as const;

// ── Beat table (30fps) ───────────────────────────────────────
const T1 = 14; // update_plan_step [step=7] tool line
const CHECK7 = 32; // sub-step #7 flips done (tally 4/7)
const T1B = 48; // update_plan_step [step=3] tool line
const CHECK3 = 66; // step #3 flips done, APP-42 closes (tally 5/7)
const R1 = 80; // typed reply
const DIV = 130; // context divider
const T2 = 144; // get_plan tool line
const R2 = 166; // typed resume reply
const NEXT = 190; // step #4 becomes the active one
const FADE = 222; // loop-seam fade

type Step = {
  id: number;
  depth: 0 | 1;
  title: string;
  issue?: string;
  done0?: boolean; // done before the video starts
};

const STEPS: Step[] = [
  {
    id: 1,
    depth: 0,
    title: "Schema migration for pending ops",
    issue: "APP-39",
    done0: true,
  },
  {
    id: 2,
    depth: 0,
    title: "Write-ahead op queue",
    issue: "APP-40",
    done0: true,
  },
  { id: 3, depth: 0, title: "Conflict resolution", issue: "APP-42" },
  { id: 6, depth: 1, title: "Detect conflicting ops", done0: true },
  { id: 7, depth: 1, title: "Last-write-wins merge" },
  { id: 4, depth: 0, title: "Retry with exponential backoff", issue: "APP-43" },
  { id: 5, depth: 0, title: "Feature flag and rollout notes" },
];
const TOTAL_STEPS = STEPS.length; // 7 discrete boxes to tick

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
        fontSize: 23,
        color: TUI.dim,
        opacity: t,
        whiteSpace: "pre",
      }}
    >
      <span>&#9881; </span>
      {children}
      <span style={{ color: C.success, opacity: okIn, fontWeight: 600 }}>
        {" "}
        &#10003;
      </span>
    </div>
  );
};

const Typed: React.FC<{ at: number; text: string }> = ({ at, text }) => {
  const frame = useCurrentFrame();
  const chars =
    frame >= at ? Math.min(text.length, Math.floor((frame - at) * 1.5)) : 0;
  if (chars === 0) return null;
  return (
    <div style={{ fontFamily: MONO, fontSize: 22, color: TUI.text }}>
      {text.slice(0, chars)}
      {chars < text.length ? (
        <span
          style={{
            display: "inline-block",
            width: 11,
            height: 22,
            marginLeft: 2,
            backgroundColor: "#c8cdd8",
            verticalAlign: "text-bottom",
          }}
        />
      ) : null}
    </div>
  );
};

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

export const PlanSync: React.FC = () => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();

  const panelIn = spring({
    frame,
    fps,
    config: { damping: 200, stiffness: 90 },
  });

  // Completion pops.
  const pop7 = spring({
    frame: frame - CHECK7,
    fps,
    config: { damping: 12, stiffness: 200, mass: 0.7 },
  });
  const pop3 = spring({
    frame: frame - CHECK3,
    fps,
    config: { damping: 12, stiffness: 200, mass: 0.7 },
  });
  const done7 = frame >= CHECK7;
  const done3 = frame >= CHECK3;
  const glow7 = frame >= CHECK7 ? Math.max(0, 1 - (frame - CHECK7) / 34) : 0;
  const glow3 = frame >= CHECK3 ? Math.max(0, 1 - (frame - CHECK3) / 40) : 0;

  // Discrete tally: 3 done at open, sub-step ticks it to 4, parent to 5.
  const doneCount = 3 + (done7 ? 1 : 0) + (done3 ? 1 : 0);

  // Loop-seam fade.
  const fade = interpolate(frame, [FADE, PLAN_SYNC_FRAMES - 2], [0, 1], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
  });

  const stepState = (s: Step): "done" | "active" | "open" => {
    if (s.done0) return "done";
    if (s.id === 7) return done7 ? "done" : "active";
    if (s.id === 3) return done3 ? "done" : "active";
    if (s.id === 4 && frame >= NEXT) return "active";
    return "open";
  };

  return (
    <AbsoluteFill
      style={{
        backgroundColor: C.bg,
        flexDirection: "row",
        justifyContent: "center",
        alignItems: "center",
        gap: 44,
      }}
    >
      {/* Agent terminal */}
      <div
        style={{
          width: 930,
          height: 510,
          borderRadius: 16,
          border: `1px solid ${C.border}`,
          backgroundColor: TUI.bg,
          boxShadow: "0 30px 80px rgba(0,0,0,0.55)",
          padding: "30px 36px",
          display: "flex",
          flexDirection: "column",
          gap: 16,
          boxSizing: "border-box",
          opacity: panelIn,
          transform: `translateY(${(1 - panelIn) * 20}px)`,
        }}
      >
        <SessionChip at={2}>agent session · wednesday, 2:14 am</SessionChip>
        <ToolLine at={T1}>lific_update_plan_step [step=7, done=true]</ToolLine>
        <ToolLine at={T1B}>lific_update_plan_step [step=3, done=true]</ToolLine>
        <Typed
          at={R1}
          text="Step 3 and its substeps are done. Lific closed APP-42."
        />

        {/* new-session divider */}
        {frame >= DIV ? (
          <div
            style={{
              display: "flex",
              alignItems: "center",
              gap: 14,
              opacity: interpolate(frame, [DIV, DIV + 10], [0, 1], {
                extrapolateLeft: "clamp",
                extrapolateRight: "clamp",
              }),
            }}
          >
            <div
              style={{ flex: 1, height: 1, backgroundColor: `${TUI.dim}55` }}
            />
            <span style={{ fontFamily: MONO, fontSize: 17, color: TUI.dim }}>
              new session &middot; fresh context
            </span>
            <div
              style={{ flex: 1, height: 1, backgroundColor: `${TUI.dim}55` }}
            />
          </div>
        ) : null}

        {frame >= DIV ? (
          <>
            <SessionChip at={DIV + 6}>
              agent session &middot; thursday, 9:05 am
            </SessionChip>
            <ToolLine at={T2}>lific_get_plan [plan=APP-PLAN-2]</ToolLine>
            <Typed
              at={R2}
              text="Resuming: step 4, retry with exponential backoff."
            />
          </>
        ) : null}
      </div>

      {/* The plan, live in Lific */}
      <div
        style={{
          width: 700,
          borderRadius: 16,
          border: `1px solid ${C.border}`,
          backgroundColor: C.bgSubtle,
          boxShadow: "0 30px 80px rgba(0,0,0,0.55)",
          padding: "28px 32px",
          boxSizing: "border-box",
          opacity: panelIn,
          transform: `translateY(${(1 - panelIn) * 20}px)`,
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
              fontSize: 15,
              color: C.accent,
              backgroundColor: C.accentSubtle,
              borderRadius: 6,
              padding: "3px 10px",
            }}
          >
            APP-PLAN-2
          </span>
          <span
            style={{
              fontFamily: DISPLAY,
              fontSize: 27,
              fontWeight: 600,
              color: C.text,
            }}
          >
            Ship offline sync
          </span>
          <span
            style={{
              marginLeft: "auto",
              fontFamily: MONO,
              fontSize: 15,
              color: C.textFaint,
            }}
          >
            {doneCount}/{TOTAL_STEPS} steps done
          </span>
        </div>

        <div style={{ display: "flex", flexDirection: "column", gap: 14 }}>
          {STEPS.map((s) => {
            const state = stepState(s);
            const sub = s.depth === 1;
            const popping = (s.id === 7 && done7) || (s.id === 3 && done3);
            const pop = s.id === 7 ? pop7 : pop3;
            const glow = s.id === 7 ? glow7 : s.id === 3 ? glow3 : 0;
            const iconSize = sub ? 18 : 22;
            return (
              <div
                key={s.id}
                style={{
                  display: "flex",
                  alignItems: "center",
                  gap: sub ? 12 : 14,
                  marginLeft: sub ? 36 : 0,
                  borderRadius: 8,
                  padding: "3px 10px",
                  margin: `-3px -10px -3px ${sub ? 26 : -10}px`,
                  backgroundColor:
                    glow > 0 ? `rgba(74,222,128,${glow * 0.1})` : undefined,
                }}
              >
                {sub ? (
                  <span
                    style={{
                      fontFamily: MONO,
                      fontSize: 16,
                      color: C.textFaint,
                    }}
                  >
                    &#9492;
                  </span>
                ) : null}
                <span
                  style={{
                    display: "inline-flex",
                    transform: popping
                      ? `scale(${1 + (1 - pop) * 0.5})`
                      : undefined,
                  }}
                >
                  {state === "done" ? (
                    <CircleCheckBig size={iconSize} color={C.success} />
                  ) : state === "active" ? (
                    <CircleDot size={iconSize} color={C.accent} />
                  ) : (
                    <Circle size={iconSize} color={C.textMuted} />
                  )}
                </span>
                <span
                  style={{
                    fontFamily: BODY,
                    fontSize: sub ? 19 : 22,
                    color: state === "done" ? C.textMuted : C.text,
                    textDecoration: state === "done" ? "line-through" : "none",
                  }}
                >
                  {s.title}
                </span>
                {s.issue ? (
                  <span
                    style={{
                      marginLeft: "auto",
                      fontFamily: MONO,
                      fontSize: 14,
                      whiteSpace: "nowrap",
                      color:
                        state === "done"
                          ? C.success
                          : state === "active"
                            ? C.accent
                            : C.textFaint,
                      backgroundColor:
                        state === "done"
                          ? "#142a1b"
                          : state === "active"
                            ? C.accentSubtle
                            : C.surface,
                      borderRadius: 6,
                      padding: "3px 9px",
                    }}
                  >
                    {s.issue} &middot; {state}
                  </span>
                ) : null}
              </div>
            );
          })}
        </div>
      </div>

      {/* Loop-seam fade */}
      <AbsoluteFill
        style={{ backgroundColor: C.bg, opacity: fade, pointerEvents: "none" }}
      />
    </AbsoluteFill>
  );
};

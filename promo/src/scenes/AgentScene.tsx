import React from "react";
import {
  AbsoluteFill,
  useCurrentFrame,
  useVideoConfig,
  spring,
  interpolate,
} from "remotion";
import { C } from "../theme";
import { BODY } from "../fonts";
import { Background } from "../components/Background";
import {
  ColumnHeader,
  IssueCard,
  Label,
  CARD_W,
  CARD_PAD,
  COL_W,
} from "../components/lific-ui";
import { OpenCodeTUI } from "../components/opencode-ui";

/*
 * The differentiator: an AI coding agent drives the tracker over MCP,
 * and a live crop of the real board reacts. The left panel is a
 * pixel-faithful OpenCode TUI replica (see components/opencode-ui.tsx);
 * the right panel is the same pixel-faithful board kit as UIScene.
 */

const L: Record<string, Label> = {
  core: { name: "core", color: "#9287d7" },
  mcp: { name: "mcp", color: "#b48af0" },
  auth: { name: "auth", color: "#fb923c" },
  bug: { name: "bug", color: "#f87171" },
};

const USER_FROM = 6; // user message appears
const THINK = 22; // Thought + thinking line
const TOOL_1 = 42; // ⚙ lific_update_issue
const BOARD_1 = 56; // LIF-198 appears in Done
const TOOL_2 = 88; // ⚙ lific_create_issue
const BOARD_2 = 104; // LIF-232 pops into Todo
const REPLY = 126; // assistant reply
const COMPLETE = 162; // ▣ Build · Agent · 4.1s — after the reply finishes streaming
const CAPTION = 40;

export const AgentScene: React.FC = () => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();

  const doneIn = spring({
    frame: frame - BOARD_1,
    fps,
    config: { damping: 16, stiffness: 140 },
  });
  const doneFlash =
    frame >= BOARD_1 ? Math.max(0, 1 - (frame - BOARD_1) / 40) : 0;
  const newIn = spring({
    frame: frame - BOARD_2,
    fps,
    config: { damping: 15, stiffness: 130 },
  });
  const newFlash =
    frame >= BOARD_2 ? Math.max(0, 1 - (frame - BOARD_2) / 40) : 0;

  // LIF-226 shifts down when LIF-232 lands on top of Todo.
  const shift = spring({
    frame: frame - BOARD_2,
    fps,
    config: { damping: 200, stiffness: 140 },
  });
  // Existing done cards shift down when LIF-198 lands on top of Done.
  const doneShift = spring({
    frame: frame - BOARD_1,
    fps,
    config: { damping: 200, stiffness: 140 },
  });

  const captionIn = interpolate(frame, [CAPTION, CAPTION + 16], [0, 1], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
  });

  const doneCount = frame >= BOARD_1 ? 3 : 2;
  const todoCount = frame >= BOARD_2 ? 2 : 1;

  return (
    <Background>
      <AbsoluteFill
        style={{
          justifyContent: "center",
          alignItems: "center",
          paddingBottom: 70,
        }}
      >
        <div
          style={{
            display: "flex",
            flexDirection: "row",
            justifyContent: "center",
            alignItems: "center",
            gap: 42,
            transform: "scale(1.16)",
          }}
        >
          {/* Agent chat panel — pixel-faithful OpenCode TUI */}
          <OpenCodeTUI
            width={880}
            height={750}
            userText="Close out the WAL race fix and file a follow-up for login rate-limiting."
            userFrom={USER_FROM}
            thought="Thought: 312ms"
            thinking="I'll close LIF-198 over Lific's MCP server and file the follow-up."
            thinkAt={THINK}
            tool1="lific_update_issue [identifier=LIF-198, status=done]"
            tool1At={TOOL_1}
            tool2="lific_create_issue [title=Rate-limit login endpoint]"
            tool2At={TOOL_2}
            reply="Done. LIF-198 closed, follow-up filed as LIF-232."
            replyAt={REPLY}
            completeAt={COMPLETE}
            completeText=" · Agent · 4.1s"
          />

          {/* Live crop of the real board: Todo + Done columns */}
          <div
            style={{
              width: COL_W * 2 + 2,
              height: 750,
              borderRadius: 16,
              border: `1px solid ${C.border}`,
              backgroundColor: C.bg,
              boxShadow: "0 30px 80px rgba(0,0,0,0.55)",
              overflow: "hidden",
              display: "flex",
              position: "relative",
            }}
          >
            {/* Todo column */}
            <div
              style={{
                width: COL_W,
                flexShrink: 0,
                borderRight: `1px solid ${C.border}`,
                boxSizing: "border-box",
                position: "relative",
              }}
            >
              <ColumnHeader status="todo" count={todoCount} />
              {/* LIF-232 pops in on create_issue */}
              {frame >= BOARD_2 ? (
                <div
                  style={{
                    position: "absolute",
                    left: CARD_PAD,
                    top: 40 + CARD_PAD,
                    opacity: newIn,
                    transform: `scale(${0.92 + newIn * 0.08}) translateY(${(1 - newIn) * -14}px)`,
                  }}
                >
                  <div
                    style={{
                      borderRadius: 6,
                      boxShadow:
                        newFlash > 0
                          ? `0 0 ${18 * newFlash}px ${C.success}66`
                          : undefined,
                    }}
                  >
                    <IssueCard
                      issue={{
                        identifier: "LIF-232",
                        title: "Rate-limit login endpoint",
                        priority: "high",
                        labels: [L.auth],
                        updated: "just now",
                      }}
                      width={CARD_W}
                    />
                  </div>
                </div>
              ) : null}
              {/* LIF-226 shifts down to make room */}
              <div
                style={{
                  position: "absolute",
                  left: CARD_PAD,
                  top: 40 + CARD_PAD + (frame >= BOARD_2 ? shift * 95 : 0),
                }}
              >
                <IssueCard
                  issue={{
                    identifier: "LIF-226",
                    title: "MCP: recurring plan templates",
                    priority: "medium",
                    labels: [L.mcp],
                    updated: "2h ago",
                  }}
                  width={CARD_W}
                />
              </div>
            </div>

            {/* Done column */}
            <div
              style={{
                width: COL_W,
                flexShrink: 0,
                boxSizing: "border-box",
                position: "relative",
              }}
            >
              <ColumnHeader status="done" count={doneCount} />
              {/* LIF-198 lands in Done on update_issue */}
              {frame >= BOARD_1 ? (
                <div
                  style={{
                    position: "absolute",
                    left: CARD_PAD,
                    top: 40 + CARD_PAD,
                    opacity: doneIn,
                    transform: `scale(${0.92 + doneIn * 0.08})`,
                  }}
                >
                  <div
                    style={{
                      borderRadius: 6,
                      boxShadow:
                        doneFlash > 0
                          ? `0 0 ${18 * doneFlash}px ${C.success}66`
                          : undefined,
                    }}
                  >
                    <IssueCard
                      issue={{
                        identifier: "LIF-198",
                        title: "Fix WAL checkpoint race on shutdown",
                        priority: "high",
                        labels: [L.core, L.bug],
                        updated: "just now",
                        status: "done",
                      }}
                      width={CARD_W}
                    />
                  </div>
                </div>
              ) : null}
              {/* Existing done cards shift down (uniform 87px cards + 8 gap) */}
              <div
                style={{
                  position: "absolute",
                  left: CARD_PAD,
                  top: 40 + CARD_PAD + (frame >= BOARD_1 ? doneShift * 95 : 0),
                  display: "flex",
                  flexDirection: "column",
                  gap: 8,
                }}
              >
                <IssueCard
                  issue={{
                    identifier: "LIF-183",
                    title: "OAuth device flow for CLI login",
                    labels: [L.auth],
                    updated: "5h ago",
                    status: "done",
                  }}
                  width={CARD_W}
                />
                <IssueCard
                  issue={{
                    identifier: "LIF-171",
                    title: "Backup retention config",
                    labels: [L.core],
                    updated: "1d ago",
                    status: "done",
                  }}
                  width={CARD_W}
                />
              </div>
            </div>
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
          fontSize: 44,
          fontWeight: 500,
          color: C.text,
          textShadow: "0 4px 30px rgba(0,0,0,0.9)",
          opacity: captionIn,
        }}
      >
        Your coding agents are first-class citizens.{" "}
        <span style={{ color: C.accent, fontWeight: 600 }}>MCP built in.</span>
      </div>
    </Background>
  );
};

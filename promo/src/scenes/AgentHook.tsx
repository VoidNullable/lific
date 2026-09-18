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
import { OpenCodeTUI } from "../components/opencode-ui";
import {
  ColumnHeader,
  IssueCard,
  Label,
  CARD_W,
  CARD_PAD,
  COL_W,
} from "../components/lific-ui";

/*
 * Ad B cold open: the product IS the hook. No title card, no problem
 * setup - frame one is an agent driving a live board. Beats condensed
 * from AgentScene; captions overlay instead of following.
 */

const L: Record<string, Label> = {
  core: { name: "core", color: "#9287d7" },
  mcp: { name: "mcp", color: "#b48af0" },
  auth: { name: "auth", color: "#fb923c" },
  bug: { name: "bug", color: "#f87171" },
};

const TOOL_1 = 14;
const BOARD_1 = 28;
const TOOL_2 = 97;
const BOARD_2 = 111;
const REPLY = 132;

export const AgentHook: React.FC = () => {
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
  const shift = spring({
    frame: frame - BOARD_2,
    fps,
    config: { damping: 200, stiffness: 140 },
  });
  const doneShift = spring({
    frame: frame - BOARD_1,
    fps,
    config: { damping: 200, stiffness: 140 },
  });

  const cap1 = interpolate(frame, [22, 36], [0, 1], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
  });
  const cap2 = interpolate(frame, [116, 130], [0, 1], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
  });

  const doneCount = frame >= BOARD_1 ? 3 : 2;
  const todoCount = frame >= BOARD_2 ? 2 : 1;

  return (
    <Background>
      <AbsoluteFill
        style={{
          flexDirection: "row",
          justifyContent: "center",
          alignItems: "center",
          gap: 42,
          paddingBottom: 90,
          transform: "scale(1.12)",
        }}
      >
        <OpenCodeTUI
          width={880}
          height={640}
          userText="Close out the WAL race fix and file a follow-up for login rate-limiting."
          userFrom={0}
          thought="Thought: 312ms"
          thinking="I'll close LIF-198 over Lific's MCP server and file the follow-up."
          thinkAt={4}
          tool1="lific_update_issue [identifier=LIF-198, status=done]"
          tool1At={TOOL_1}
          tool2="lific_create_issue [title=Rate-limit login endpoint]"
          tool2At={TOOL_2}
          reply="Done. LIF-198 closed, follow-up filed as LIF-232."
          replyAt={REPLY}
          completeAt={182}
          completeText=" · Agent · 4.1s"
        />

        {/* Live board crop */}
        <div
          style={{
            width: COL_W * 2 + 2,
            height: 640,
            borderRadius: 16,
            border: `1px solid ${C.border}`,
            backgroundColor: C.bg,
            boxShadow: "0 30px 80px rgba(0,0,0,0.55)",
            overflow: "hidden",
            display: "flex",
          }}
        >
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
            {frame >= BOARD_2 ? (
              <div
                style={{
                  position: "absolute",
                  left: CARD_PAD,
                  top: 48,
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
            <div
              style={{
                position: "absolute",
                left: CARD_PAD,
                top: 48 + (frame >= BOARD_2 ? shift * 95 : 0),
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
          <div
            style={{
              width: COL_W,
              flexShrink: 0,
              boxSizing: "border-box",
              position: "relative",
            }}
          >
            <ColumnHeader status="done" count={doneCount} />
            {frame >= BOARD_1 ? (
              <div
                style={{
                  position: "absolute",
                  left: CARD_PAD,
                  top: 48,
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
            <div
              style={{
                position: "absolute",
                left: CARD_PAD,
                top: 48 + (frame >= BOARD_1 ? doneShift * 95 : 0),
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
      </AbsoluteFill>

      {/* Overlay hook captions */}
      <div
        style={{
          position: "absolute",
          bottom: 92,
          width: "100%",
          textAlign: "center",
          fontFamily: BODY,
          fontSize: 46,
          fontWeight: 600,
          color: C.text,
          opacity: cap1,
          textShadow: "0 4px 30px rgba(0,0,0,0.95)",
        }}
      >
        Your coding agents, running their own issue tracker.
      </div>
      <div
        style={{
          position: "absolute",
          bottom: 40,
          width: "100%",
          textAlign: "center",
          fontFamily: MONO,
          fontSize: 30,
          color: C.accent,
          opacity: cap2,
          textShadow: "0 4px 30px rgba(0,0,0,0.95)",
        }}
      >
        live, over MCP
      </div>
    </Background>
  );
};

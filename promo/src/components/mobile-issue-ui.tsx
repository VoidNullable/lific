import React from "react";
import { C } from "../theme";
import { BODY, DISPLAY, MONO } from "../fonts";
import { ChevronRight, ChevronDown, Circle, History } from "./icons";
import {
  Menu,
  Pencil,
  Eye,
  Download,
  Ellipsis,
  PanelRight,
  MessageSquare,
} from "./lucide-extra";

/*
 * The issue view on a 390px phone, dark theme, description in preview
 * mode: Layout.svelte's mobile header, the issue topbar from
 * IssueDetail.svelte, a Markdown.svelte task list, ActivityTimeline.svelte
 * and the top of Comments.svelte. Positions are the computed CSS px of the
 * running app at 390x844, measured after ticking a task checkbox.
 *
 * `inset` is env(safe-area-inset-top): the app draws under the status bar
 * and pads its header by that much, so everything below shifts down.
 */

const SUBTLE_PILL = C.bgSubtle;
const INSET_SHADOW = "inset 0 1px 2px rgba(0,0,0,0.04)";

export type Task = { text: string; done: boolean };
export type TimelineEntry = {
  actor: string;
  agent?: boolean;
  verb: string;
  /** Rendered after the verb in --text, e.g. "@blake". */
  object?: string;
  /** ActivityTimeline's diff toggle for description changes. */
  showChange?: boolean;
  when: string;
  via: string;
};

/**
 * Chromium's native checkbox at 13px, light color-scheme: the app leaves
 * task-list inputs unstyled, so this is what a reader sees and taps.
 */
const NativeCheckbox: React.FC<{ checked: boolean }> = ({ checked }) =>
  checked ? (
    <svg width={13} height={13} viewBox="0 0 13 13" style={{ display: "block" }}>
      <rect x={0} y={0} width={13} height={13} rx={2} fill="#0075ff" />
      <path
        d="M3 6.6 5.4 9 10 4.2"
        fill="none"
        stroke="#ffffff"
        strokeWidth={2}
        strokeLinecap="square"
        strokeLinejoin="miter"
      />
    </svg>
  ) : (
    <svg width={13} height={13} viewBox="0 0 13 13" style={{ display: "block" }}>
      <rect x={0.5} y={0.5} width={12} height={12} rx={2} fill="#ffffff" stroke="#767676" />
    </svg>
  );

const AgentBadge: React.FC = () => (
  <span
    style={{
      display: "inline-block",
      verticalAlign: "middle",
      margin: "0 2px",
      padding: "1px 4px",
      borderRadius: 4,
      backgroundColor: C.accentSubtle,
      color: C.accent,
      fontSize: 11,
      lineHeight: "17.875px",
      fontWeight: 600,
      letterSpacing: "0.05em",
      textTransform: "uppercase",
    }}
  >
    agent
  </span>
);

const at = (x: number, y: number, extra?: React.CSSProperties): React.CSSProperties => ({
  position: "absolute",
  left: x,
  top: y,
  ...extra,
});

export const MobileIssueView: React.FC<{
  width?: number;
  height: number;
  inset: number;
  project: { identifier: string; name: string };
  identifier: string;
  status: string;
  title: string;
  tasks: Task[];
  timeline: TimelineEntry[];
}> = ({ width = 390, height, inset, project, identifier, status, title, tasks, timeline }) => {
  const MAIN = 100; // header 48 + issue topbar 52
  const LIST_TOP = 86.2; // title button + mb-4 + ul margin-top
  const LI_PITCH = 27.3; // 23.8 line + 0.25em margin
  const listBottom = LIST_TOP + tasks.length * LI_PITCH + 10.5;
  const ACT = listBottom + 36.5; // activity heading
  const ENTRY0 = ACT + 42.6; // first timeline entry
  const ENTRY_PITCH = 55;
  const lastEntry = ENTRY0 + (timeline.length - 1) * ENTRY_PITCH;
  const COMMENTS = lastEntry + 43 + 40.2; // section rule

  return (
    <div
      style={{
        position: "relative",
        width,
        height,
        overflow: "hidden",
        backgroundColor: C.bg,
        fontFamily: BODY,
        color: C.text,
      }}
    >
      {/* Mobile header, padded by the safe-area inset */}
      <div style={at(0, 0, { width, height: inset + 48, backgroundColor: C.chrome })}>
        <div style={at(16, inset + 14)}>
          <Menu size={20} color={C.textMuted} />
        </div>
        <span
          style={at(58, inset + 12, {
            width: 24,
            height: 24,
            borderRadius: 4,
            display: "grid",
            placeItems: "center",
            backgroundColor: "#232926",
            color: C.textMuted,
            fontSize: 11,
            fontWeight: 500,
            letterSpacing: "-0.025em",
          })}
        >
          {project.identifier.slice(0, 2)}
        </span>
        <div style={at(90, inset + 12, { display: "flex", alignItems: "baseline", gap: 6, height: 24 })}>
          <span
            style={{
              fontFamily: DISPLAY,
              fontSize: 15,
              lineHeight: "24px",
              letterSpacing: "-0.025em",
              color: C.text,
            }}
          >
            {project.name}
          </span>
          <span style={{ fontSize: 13, lineHeight: "20.8px", color: C.textFaint }}>Issues</span>
          <span style={{ marginLeft: 2, alignSelf: "center" }}>
            <ChevronRight size={14} color={C.textFaint} />
          </span>
        </div>
      </div>

      {/* Issue topbar: identifier / status, edit-preview toggle, export, more, details */}
      <div style={at(0, inset + 48, { width, height: 52, backgroundColor: C.chrome })}>
        <div style={at(12, 0, { height: 52, display: "flex", alignItems: "center", gap: 6 })}>
          <span style={{ fontFamily: MONO, fontSize: 13, lineHeight: "20.8px", fontWeight: 500, color: C.text }}>
            {identifier}
          </span>
          <span style={{ fontSize: 16, lineHeight: "25.6px", color: C.textFaint }}>/</span>
          <Circle size={13} color={C.textMuted} />
          <span style={{ fontSize: 13, lineHeight: "20.8px", color: C.textMuted, textTransform: "capitalize" }}>
            {status}
          </span>
          <ChevronDown size={11} color={C.textFaint} />
        </div>
        <div
          style={at(170, 8, {
            width: 84,
            height: 36,
            boxSizing: "border-box",
            borderRadius: 999,
            border: `1px solid ${C.border}`,
            backgroundColor: SUBTLE_PILL,
            boxShadow: INSET_SHADOW,
          })}
        >
          <div style={at(15, 10)}>
            <Pencil size={14} color={C.textMuted} />
          </div>
          <div
            style={at(41, 3, {
              width: 38,
              height: 28,
              borderRadius: 999,
              backgroundColor: C.surface,
              boxShadow: "0 1px 2px rgba(0,0,0,0.06), 0 1px 3px rgba(0,0,0,0.08)",
              display: "grid",
              placeItems: "center",
            })}
          >
            <Eye size={14} color={C.text} />
          </div>
        </div>
        <div
          style={at(258, 11, {
            width: 48,
            height: 30,
            boxSizing: "border-box",
            borderRadius: 999,
            border: `1px solid ${C.border}`,
            backgroundColor: SUBTLE_PILL,
            boxShadow: INSET_SHADOW,
            display: "grid",
            placeItems: "center",
          })}
        >
          <Download size={14} color={C.textMuted} />
        </div>
        <div style={at(317, 19)}>
          <Ellipsis size={14} color={C.textFaint} />
        </div>
        <div style={at(352, 18)}>
          <PanelRight size={16} color={C.textMuted} />
        </div>
      </div>

      {/* Recessed content */}
      <div style={at(0, inset + MAIN, { width, bottom: 0, backgroundColor: C.bg })}>
        <div
          style={{
            fontFamily: DISPLAY,
            position: "absolute",
            left: 16,
            top: 24,
            fontSize: 22,
            lineHeight: "35.2px",
            letterSpacing: "-0.025em",
            color: C.text,
          }}
        >
          {title}
        </div>

        {/* Description: a GFM task list (Markdown.svelte, .prose ul) */}
        {tasks.map((t, i) => {
          const y = LIST_TOP + i * LI_PITCH;
          return (
            <div key={t.text} style={at(0, y, { width, height: 23.8 })}>
              <span
                style={at(21, 9.4, {
                  width: 5,
                  height: 5,
                  borderRadius: 3,
                  backgroundColor: C.text,
                })}
              />
              <div style={at(37, 3)}>
                <NativeCheckbox checked={t.done} />
              </div>
              <span style={at(53.6, 0, { fontSize: 14, lineHeight: "23.8px", whiteSpace: "nowrap" })}>
                {t.text}
              </span>
            </div>
          );
        })}

        {/* Activity (ActivityTimeline.svelte) */}
        <div
          style={at(16, ACT, {
            width: 358,
            height: 26.6,
            boxSizing: "border-box",
            borderBottom: `1px solid ${C.border}`,
          })}
        >
          <div style={at(0, 2.3)}>
            <History size={13} color={C.textFaint} />
          </div>
          <span
            style={at(21, 2.2, {
              fontFamily: DISPLAY,
              fontSize: 11,
              lineHeight: "13.2px",
              fontWeight: 600,
              letterSpacing: "0.1em",
              textTransform: "uppercase",
              color: C.textMuted,
            })}
          >
            Activity
          </span>
          <span style={at(83.8, 0, { fontSize: 11, lineHeight: "17.6px", color: C.textFaint })}>
            {timeline.length}
          </span>
        </div>

        <div
          style={at(19, ENTRY0 + 6, {
            width: 1,
            height: lastEntry + 43 - 6 - (ENTRY0 + 6),
            backgroundColor: C.border,
          })}
        />
        {timeline.map((e, i) => {
          const y = ENTRY0 + i * ENTRY_PITCH;
          return (
            <React.Fragment key={`${e.actor}-${e.verb}`}>
              <span
                style={at(16, y + 7, {
                  width: 7,
                  height: 7,
                  boxSizing: "border-box",
                  borderRadius: 4,
                  border: `1px solid ${C.border}`,
                  backgroundColor: C.surface,
                })}
              />
              <div
                style={at(36, y, {
                  width: 338,
                  fontSize: 13,
                  lineHeight: "21.125px",
                  color: C.textMuted,
                })}
              >
                <div style={{ whiteSpace: "nowrap" }}>
                  <span style={{ fontWeight: 500, color: C.text }}>{e.actor}</span>{" "}
                  {e.agent ? (
                    <>
                      <AgentBadge />{" "}
                    </>
                  ) : null}
                  {e.verb}
                  {e.object ? (
                    <>
                      {" "}
                      <span style={{ color: C.text }}>{e.object}</span>
                    </>
                  ) : null}
                  {e.showChange ? (
                    <span style={{ marginLeft: 5, fontSize: 12, color: C.accent }}>
                      show change
                      <span style={{ display: "inline-block", marginLeft: 4, verticalAlign: "-1px" }}>
                        <ChevronDown size={11} color={C.accent} />
                      </span>
                    </span>
                  ) : null}
                </div>
                <div style={{ fontSize: 12, lineHeight: "19.5px", color: C.textFaint, marginTop: 1.4 }}>
                  · {e.when} via {e.via}
                </div>
              </div>
            </React.Fragment>
          );
        })}

        {/* Comments (Comments.svelte), empty state and composer */}
        <div style={at(16, COMMENTS, { width: 358, borderTop: `1px solid ${C.border}` })} />
        <div
          style={at(16, COMMENTS + 33, {
            fontFamily: DISPLAY,
            fontSize: 17,
            lineHeight: "20.4px",
            fontWeight: 600,
            letterSpacing: "-0.01em",
          })}
        >
          Comments
        </div>
        <span
          style={at(16, COMMENTS + 82.9, {
            width: 32,
            height: 32,
            borderRadius: 999,
            backgroundColor: C.bgSubtle,
            display: "grid",
            placeItems: "center",
          })}
        >
          <MessageSquare size={16} color={C.textMuted} />
        </span>
        <div style={at(60, COMMENTS + 77.3, { fontSize: 14, lineHeight: "22.4px", fontWeight: 500 })}>
          No comments yet
        </div>
        <div style={at(60, COMMENTS + 99.7, { fontSize: 13, lineHeight: "20.8px", color: C.textMuted })}>
          Start the conversation below.
        </div>
        <div
          style={at(16, COMMENTS + 148.5, {
            width: 358,
            height: 180,
            boxSizing: "border-box",
            borderRadius: 12,
            border: `1px solid ${C.border}`,
            backgroundColor: C.surface,
          })}
        >
          {/* 16px on phones so focusing the field doesn't zoom the page */}
          <div style={at(16, 14, { fontSize: 16, lineHeight: "25.6px", color: C.textMuted })}>
            Write a comment…
          </div>
          <div style={at(0, 80.1, { width: 356, borderTop: `1px solid ${C.border}` })} />
          <div style={at(106, 89.1, { width: 68, fontSize: 12, lineHeight: "19.2px", color: C.textMuted })}>
            Markdown · drag, paste or attach files
          </div>
        </div>
      </div>
    </div>
  );
};

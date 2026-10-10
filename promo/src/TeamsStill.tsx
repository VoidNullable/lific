import React from "react";
import { AbsoluteFill } from "remotion";
import { C } from "./theme";
import { BODY } from "./fonts";
import { PriorityIcon, StatusIcon } from "./components/lific-ui";
import { ChevronDown } from "./components/icons";
import {
  ActivityLine,
  ActivityTimeline,
  CommentThread,
  DetailComment,
} from "./components/issue-detail-ui";
import { PlanPanel, PlanStep, PlanStepState } from "./PlanSync";

/*
 * TeamsStill: the landing page's "For teams" still, two panes cut into
 * two windows on the site.
 * Left: APP-42's issue page, its activity timeline and comment thread
 * after opencode-blake closed it with evidence. One update_issue call
 * (status=done plus evidence) writes three audit rows in one transaction:
 * the status change, the linked plan step's auto-complete, and the
 * verification comment. Older rows show Ada and her Cursor.
 * Right: APP-PLAN-2 as PlanSync draws it, settled at 5/7 with APP-42's
 * step done and APP-43 not started yet.
 *
 * Both panes share a height so the site's windows line up. Render, then
 * crop each pane:
 *   bunx remotion still TeamsStill /tmp/opencode/teams-still.png
 *   magick /tmp/opencode/teams-still.png -crop 845x640+36+30 +repage -quality 90 ../site/public/teams-activity.webp
 *   magick /tmp/opencode/teams-still.png -crop 713x640+909+30 +repage -quality 90 ../site/public/teams-plan.webp
 */

// ── Geometry (image px) ──────────────────────────────────────
const S = 1.35; // the issue page renders at app CSS px, scaled up
const K = 1.15; // the plan card renders at PlanSync px, scaled up
const PLAN_W = 620; // PlanSync px; the video's card is 700
const ROW_GAP = 30; // PlanSync px; the video uses 14
const MX = 36;
const TOP = 30;
const GAP = 28;
const LEFT_W = 845;
const RIGHT_W = Math.round(PLAN_W * K);
const PANE_H = 640;

export const TEAMS_STILL_W = MX * 2 + LEFT_W + GAP + RIGHT_W;
export const TEAMS_STILL_H = TOP * 2 + PANE_H;

// ── Activity values (ActivityTimeline.svelte) ────────────────

/** A status or priority value: icon 12 + capitalized word, mx-0.5. */
const Value: React.FC<{ icon: React.ReactNode; text: string; strong?: boolean }> = ({
  icon,
  text,
  strong,
}) => (
  <span
    style={{
      display: "inline-flex",
      alignItems: "center",
      gap: 4,
      verticalAlign: "middle",
      margin: "0 2px",
    }}
  >
    {icon}
    <span style={{ textTransform: "capitalize", color: strong ? C.text : undefined }}>
      {text}
    </span>
  </span>
);

const Arrow: React.FC = () => <span style={{ color: C.textFaint }}>&rarr;</span>;

const statusChange = (from: string, to: string) => (
  <>
    {" "}changed status{" "}
    <Value icon={<StatusIcon status={from} size={12} />} text={from} />{" "}
    <Arrow />{" "}
    <Value icon={<StatusIcon status={to} size={12} />} text={to} strong />
  </>
);

/** shortValue(v, 60): newlines flattened, cut with an ellipsis. */
const shortValue = (v: string, max: number) => {
  const flat = v.replace(/\n+/g, " ").trim();
  return flat.length > max ? flat.slice(0, max) + "\u2026" : flat;
};

const EVIDENCE = "cargo test: 412 passed, 0 failed.";

// Newest first, the six the timeline shows before "Show all". The
// mimic puts no space between the agent badge and the verb, so each
// bot line opens with one, as the Svelte whitespace does.
const ACTIVITY: ActivityLine[] = [
  {
    actor: "opencode-blake",
    bot: true,
    text: (
      <>
        {" "}commented{" "}
        <span style={{ color: C.textFaint, fontStyle: "italic" }}>
          &ldquo;{shortValue(EVIDENCE, 60)}&rdquo;
        </span>
      </>
    ),
    time: "5h ago via mcp",
  },
  {
    actor: "opencode-blake",
    bot: true,
    text: " auto-completed a step (issue closed)",
    time: "5h ago via mcp",
  },
  {
    actor: "opencode-blake",
    bot: true,
    text: statusChange("active", "done"),
    time: "5h ago via mcp",
  },
  {
    actor: "cursor-ada",
    bot: true,
    text: (
      <>
        {" "}changed description{" "}
        <span
          style={{
            display: "inline-flex",
            alignItems: "center",
            gap: 2,
            fontSize: 12,
            color: C.accent,
          }}
        >
          show change
          <ChevronDown size={11} color={C.accent} />
        </span>
      </>
    ),
    time: "9h ago via mcp",
  },
  {
    actor: "ada",
    text: (
      <>
        changed priority{" "}
        <Value icon={<PriorityIcon priority="medium" size={12} />} text="medium" />{" "}
        <Arrow />{" "}
        <Value icon={<PriorityIcon priority="high" size={12} />} text="high" strong />
      </>
    ),
    time: "10h ago via web",
  },
  {
    actor: "opencode-blake",
    bot: true,
    text: statusChange("todo", "active"),
    time: "1d ago via mcp",
  },
];

// Comments.svelte prints no agent badge on a comment; the author name and
// the Verification pill carry it. Initials: "opencode-blake" -> "OB".
const COMMENTS: DetailComment[] = [
  {
    id: 118,
    author: "opencode-blake",
    initials: "OB",
    time: "5h ago",
    kind: "verification",
    body: EVIDENCE,
  },
];

// ── Composition ──────────────────────────────────────────────

export const TeamsStill: React.FC = () => (
  <AbsoluteFill style={{ backgroundColor: C.bg }}>
    {/* APP-42: the issue page's activity and comments */}
    <div
      style={{
        position: "absolute",
        left: MX,
        top: TOP,
        width: LEFT_W,
        height: PANE_H,
        overflow: "hidden",
        backgroundColor: C.bg,
      }}
    >
      <div
        style={{
          width: LEFT_W / S,
          // The timeline's own mt-10 sits above the crop; this nets the
          // main column's py-6 at the top edge.
          padding: "0 32px",
          marginTop: -16,
          boxSizing: "border-box",
          transform: `scale(${S})`,
          transformOrigin: "top left",
          fontFamily: BODY,
        }}
      >
        <ActivityTimeline items={ACTIVITY} total={9} />
        <CommentThread comments={COMMENTS} />
      </div>
    </div>

    {/* APP-PLAN-2: the plan card from PlanSync, chrome off, its rows
        spaced out and centred to fill the shared pane height */}
    <div
      style={{
        position: "absolute",
        left: MX + LEFT_W + GAP,
        top: TOP,
        width: RIGHT_W,
        height: PANE_H,
        overflow: "hidden",
        display: "flex",
        alignItems: "center",
        backgroundColor: C.bgSubtle,
      }}
    >
      <div style={{ width: PLAN_W, zoom: K }}>
        <PlanPanel
          doneCount={5}
          stepState={(s: PlanStep): PlanStepState =>
            s.id === 4 || s.id === 5 ? "open" : "done"
          }
          rowGap={ROW_GAP}
          style={{
            width: PLAN_W,
            borderRadius: 0,
            border: "none",
            boxShadow: "none",
          }}
        />
      </div>
    </div>
  </AbsoluteFill>
);

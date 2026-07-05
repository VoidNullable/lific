import React from "react";
import { C } from "../theme";
import { BODY, DISPLAY, MONO } from "../fonts";
import { Sidebar, StatusIcon, PriorityIcon, IssueData, Label } from "./lific-ui";
import { ChevronRight, History, Plus } from "./icons";

/*
 * Pixel-faithful replica of the web UI's ISSUE DETAIL view —
 * web/src/routes/IssueDetail.svelte rendered through the shared
 * web/src/lib/DocumentDetail.svelte (layout="two-column"). Every size
 * below is the computed CSS px of the corresponding Tailwind class /
 * app.css token (dark mode, the default desktop view).
 *
 * Chrome topbar: DocumentDetail.svelte:496-570
 *   px-6 py-2, Breadcrumbs (LIF › Issues › LIF-198) + status badge
 *   (breadcrumbExtra, IssueDetail.svelte:470-487) on the left, an
 *   Export toolbar-pill + delete kebab on the right.
 * Content panel: DocumentDetail.svelte:387-424 (two-column)
 *   max-w-[1120px] centered; main column px-8 py-6 (sm+): InlineTitle
 *   (text-title 22px, DocumentDetail titleSize="md"), prose body
 *   (app.css .prose: 14px / 1.7), ActivityTimeline, Comments.
 *   Metadata aside: md:w-[220px], border-l, px-5 py-6 — the
 *   issue-meta-aside fields (Status / Priority / Module / Labels) +
 *   divider + Created / Updated dates.
 * Comments: Comments.svelte — avatar gutter (2rem) + connector, author +
 *   TimeAgo, markdown body; a bot author shows an "agent" badge
 *   (ActivityTimeline.svelte:165-173 vocabulary).
 */

// Type scale (app.css @theme)
const MICRO = 11;
const CAPTION = 12;
const BODY_SM = 13;
const BODY_TXT = 14;
const TITLE = 22; // --text-title (InlineTitle md)

// Canonical status text color (StatusIcon.svelte statusCssColor).
const statusTextColor = (s: string): string => {
  switch (s) {
    case "active":
      return C.accent;
    case "done":
      return C.success;
    case "todo":
      return C.textMuted;
    default:
      return C.textFaint;
  }
};

// Priority text color (IssueDetail.svelte priorityTextClass).
const priorityTextColor = (p: string): string => {
  switch (p) {
    case "urgent":
      return C.error;
    case "high":
      return C.warn;
    case "medium":
      return C.accent;
    default:
      return C.text;
  }
};

const cap = (s: string) => s.charAt(0).toUpperCase() + s.slice(1);

// ── Detail data shape ────────────────────────────────────────

export type DetailComment = {
  author: string;
  /** Two-letter initials shown in the avatar. */
  initials: string;
  time: string;
  body: string;
  /** Renders the "agent" badge next to the author (bot commenter). */
  bot?: boolean;
};

export type ActivityLine = {
  actor: string;
  /** e.g. "changed status", "created this issue". */
  text: React.ReactNode;
  time: string;
  bot?: boolean;
};

export type IssueDetailData = IssueData & {
  description: React.ReactNode;
  module?: string;
  created: string;
  updatedAbs: string;
  comments: DetailComment[];
  activity: ActivityLine[];
};

// ── Chrome topbar (DocumentDetail topbar snippet) ────────────

const Breadcrumbs: React.FC<{ identifier: string }> = ({ identifier }) => (
  <nav style={{ display: "flex", alignItems: "center", gap: 6, minWidth: 0 }}>
    {/* project scope — mono, muted */}
    <span
      style={{
        fontSize: BODY_SM,
        fontFamily: MONO,
        fontWeight: 500,
        color: C.textMuted,
      }}
    >
      LIF
    </span>
    <ChevronRight size={12} color={C.textFaint} />
    {/* parent list — links back to Issues */}
    <span style={{ fontSize: BODY_SM, fontWeight: 500, color: C.textMuted }}>
      Issues
    </span>
    <ChevronRight size={12} color={C.textFaint} />
    {/* current page — identifier, mono, strong */}
    <span
      style={{
        fontSize: BODY_SM,
        fontFamily: MONO,
        fontWeight: 500,
        color: C.text,
      }}
    >
      {identifier}
    </span>
  </nav>
);

const DetailTopbar: React.FC<{ issue: IssueData }> = ({ issue }) => (
  <div
    style={{
      height: 44, // TOPBAR_H
      display: "flex",
      alignItems: "center",
      gap: 12, // gap-3
      padding: "8px 24px", // px-6 py-2
      boxSizing: "border-box",
      backgroundColor: C.chrome,
      fontFamily: BODY,
    }}
  >
    {/* Left zone: breadcrumb trail + status badge (breadcrumbExtra). */}
    <div style={{ display: "flex", alignItems: "center", gap: 6, minWidth: 0 }}>
      <Breadcrumbs identifier={issue.identifier} />
      <span style={{ fontSize: BODY_SM, color: C.textFaint }}>/</span>
      <span style={{ display: "flex", alignItems: "center", gap: 6 }}>
        <StatusIcon status={issue.status ?? "active"} size={13} />
        <span
          style={{
            fontSize: BODY_SM,
            textTransform: "capitalize",
            color: statusTextColor(issue.status ?? "active"),
          }}
        >
          {issue.status}
        </span>
      </span>
    </div>

    {/* Right zone: saved indicator + export pill + kebab. */}
    <div
      style={{
        marginLeft: "auto",
        display: "flex",
        alignItems: "center",
        gap: 8,
      }}
    >
      <span style={{ fontSize: CAPTION, color: C.textFaint }}>
        Saved at 4:12 PM
      </span>
      {/* Export toolbar-pill (app.css .toolbar-pill). */}
      <div
        style={{
          display: "inline-flex",
          alignItems: "center",
          gap: 6,
          padding: "7px 16px",
          fontSize: BODY_SM,
          fontWeight: 500,
          color: C.textMuted,
          backgroundColor: C.bgSubtle,
          border: `1px solid ${C.border}`,
          borderRadius: 999,
          boxShadow: "inset 0 1px 2px rgba(0,0,0,0.04)",
        }}
      >
        <DownloadIcon size={14} color={C.textMuted} />
        Export
      </div>
      {/* Delete kebab. */}
      <div
        style={{
          width: 28,
          height: 28,
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          color: C.textMuted,
        }}
      >
        <KebabIcon size={16} color={C.textMuted} />
      </div>
    </div>
  </div>
);

// Two small lucide glyphs the topbar needs (Download, vertical ellipsis).
const DownloadIcon: React.FC<{ size: number; color: string }> = ({
  size,
  color,
}) => (
  <svg
    width={size}
    height={size}
    viewBox="0 0 24 24"
    fill="none"
    stroke={color}
    strokeWidth="2"
    strokeLinecap="round"
    strokeLinejoin="round"
    style={{ flexShrink: 0, display: "block" }}
  >
    <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />
    <polyline points="7 10 12 15 17 10" />
    <line x1="12" x2="12" y1="15" y2="3" />
  </svg>
);

const KebabIcon: React.FC<{ size: number; color: string }> = ({
  size,
  color,
}) => (
  <svg
    width={size}
    height={size}
    viewBox="0 0 24 24"
    fill={color}
    style={{ flexShrink: 0, display: "block" }}
  >
    <circle cx="12" cy="5" r="1.6" />
    <circle cx="12" cy="12" r="1.6" />
    <circle cx="12" cy="19" r="1.6" />
  </svg>
);

// ── Metadata sidebar fields (issue-meta-aside) ───────────────

const FieldLabel: React.FC<{ children: React.ReactNode }> = ({ children }) => (
  <p
    style={{
      margin: 0,
      fontSize: MICRO,
      fontWeight: 600,
      letterSpacing: "0.1em",
      textTransform: "uppercase",
      color: C.textFaint,
      lineHeight: 1.25,
    }}
  >
    {children}
  </p>
);

const MetaField: React.FC<{
  label: string;
  children: React.ReactNode;
}> = ({ label, children }) => (
  <div
    style={{
      display: "flex",
      flexDirection: "column",
      gap: 10, // issue-meta-field gap-0.625rem
      minWidth: 0,
    }}
  >
    <FieldLabel>{label}</FieldLabel>
    {children}
  </div>
);

const MetaSidebar: React.FC<{ issue: IssueDetailData }> = ({ issue }) => (
  <aside
    style={{
      width: 220, // md:w-[220px]
      flexShrink: 0,
      borderLeft: `1px solid ${C.border}`,
      backgroundColor: C.bg,
      padding: "24px 20px", // py-6 px-5
      boxSizing: "border-box",
      display: "flex",
      flexDirection: "column",
      gap: 22, // issue-meta-aside gap-1.375rem
      fontFamily: BODY,
    }}
  >
    <MetaField label="Status">
      <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
        <StatusIcon status={issue.status ?? "active"} size={14} />
        <span
          style={{
            fontSize: BODY_SM,
            textTransform: "capitalize",
            color: C.text,
          }}
        >
          {issue.status}
        </span>
      </div>
    </MetaField>

    <MetaField label="Priority">
      <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
        {issue.priority ? (
          <PriorityIcon priority={issue.priority} size={14} />
        ) : null}
        <span
          style={{
            fontSize: BODY_SM,
            color: priorityTextColor(issue.priority ?? "none"),
          }}
        >
          {issue.priority ? cap(issue.priority) : "No priority"}
        </span>
      </div>
    </MetaField>

    <MetaField label="Module">
      <span
        style={{
          fontSize: BODY_SM,
          color: issue.module ? C.text : C.textFaint,
        }}
      >
        {issue.module ?? "None"}
      </span>
    </MetaField>

    <MetaField label="Labels">
      <div
        style={{
          display: "flex",
          flexWrap: "wrap",
          alignItems: "center",
          gap: 6, // gap-1.5
        }}
      >
        {(issue.labels ?? []).map((lbl: Label) => (
          <span
            key={lbl.name}
            style={{
              display: "inline-flex",
              alignItems: "center",
              fontSize: CAPTION,
              fontWeight: 500,
              padding: "2px 8px", // px-2 py-0.5
              borderRadius: 999,
              border: `1px solid ${lbl.color}40`,
              background: `${lbl.color}10`,
              color: lbl.color,
              lineHeight: 1.3,
            }}
          >
            {lbl.name}
          </span>
        ))}
        {/* dashed add-label affordance (size-5 border-dashed) */}
        <span
          style={{
            width: 20,
            height: 20,
            borderRadius: 4,
            border: `1px dashed ${C.border}`,
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            color: C.textFaint,
          }}
        >
          <Plus size={12} color={C.textFaint} />
        </span>
      </div>
    </MetaField>

    {/* divider (border-t -mx-5) */}
    <div
      style={{
        height: 1,
        backgroundColor: C.border,
        margin: "-2px -20px",
      }}
    />

    {/* Dates block (issue-meta-dates gap-1rem) */}
    <div style={{ display: "flex", flexDirection: "column", gap: 16 }}>
      <MetaField label="Created">
        <p
          style={{
            margin: 0,
            fontSize: BODY_SM,
            color: C.textMuted,
            lineHeight: 1.375,
          }}
        >
          {issue.created}
        </p>
      </MetaField>
      <MetaField label="Updated">
        <p
          style={{
            margin: 0,
            fontSize: BODY_SM,
            color: C.textMuted,
            lineHeight: 1.375,
          }}
        >
          {issue.updatedAbs}
        </p>
      </MetaField>
    </div>
  </aside>
);

// ── Activity timeline (ActivityTimeline.svelte) ──────────────

const AgentBadge: React.FC = () => (
  <span
    style={{
      display: "inline-block",
      verticalAlign: "middle",
      fontSize: MICRO,
      fontWeight: 600,
      textTransform: "uppercase",
      letterSpacing: "0.05em",
      padding: "1px 4px",
      borderRadius: 4,
      backgroundColor: C.accentSubtle,
      color: C.accent,
      margin: "0 2px",
    }}
  >
    agent
  </span>
);

const ActivityTimeline: React.FC<{ items: ActivityLine[] }> = ({ items }) => (
  <section style={{ marginTop: 40 }}>
    {/* Header — uppercase-tracking, hairline underline. */}
    <div
      style={{
        display: "flex",
        alignItems: "center",
        gap: 8,
        marginBottom: 16,
        paddingBottom: 8,
        borderBottom: `1px solid ${C.border}`,
      }}
    >
      <History size={13} color={C.textFaint} />
      <span
        style={{
          fontSize: MICRO,
          fontWeight: 600,
          textTransform: "uppercase",
          letterSpacing: "0.1em",
          color: C.textMuted,
        }}
      >
        Activity
      </span>
      <span
        style={{
          fontSize: MICRO,
          color: C.textFaint,
          fontVariantNumeric: "tabular-nums",
        }}
      >
        {items.length}
      </span>
    </div>

    <ol style={{ margin: 0, padding: 0, listStyle: "none", position: "relative" }}>
      {/* Gutter rail. */}
      <div
        style={{
          position: "absolute",
          left: 3,
          top: 6,
          bottom: 6,
          width: 1,
          backgroundColor: C.border,
        }}
      />
      {items.map((a, i) => (
        <li
          key={i}
          style={{
            position: "relative",
            paddingLeft: 20,
            paddingBottom: i === items.length - 1 ? 0 : 12,
          }}
        >
          {/* Dot. */}
          <span
            style={{
              position: "absolute",
              left: 0,
              top: 7,
              width: 7,
              height: 7,
              borderRadius: 999,
              border: `1px solid ${C.border}`,
              backgroundColor: C.surface,
            }}
          />
          <div
            style={{
              fontSize: BODY_SM,
              lineHeight: 1.6,
              color: C.textMuted,
            }}
          >
            <span style={{ fontWeight: 500, color: C.text }}>{a.actor}</span>
            {a.bot ? <AgentBadge /> : " "}
            {a.text}
            <span
              style={{
                fontSize: CAPTION,
                color: C.textFaint,
                whiteSpace: "nowrap",
              }}
            >
              {" · "}
              {a.time}
            </span>
          </div>
        </li>
      ))}
    </ol>
  </section>
);

// ── Comments thread (Comments.svelte) ────────────────────────

const CommentThread: React.FC<{ comments: DetailComment[] }> = ({
  comments,
}) => (
  <section
    style={{
      marginTop: 40, // 2.5rem
      paddingTop: 32, // 2rem
      borderTop: `1px solid ${C.border}`,
    }}
  >
    <div
      style={{
        display: "flex",
        alignItems: "center",
        gap: 10,
        marginBottom: 24,
      }}
    >
      <h2
        style={{
          margin: 0,
          fontFamily: DISPLAY,
          fontSize: 17, // 1.0625rem
          fontWeight: 600,
          letterSpacing: "-0.01em",
          color: C.text,
        }}
      >
        Comments
      </h2>
      <span
        style={{
          display: "inline-flex",
          alignItems: "center",
          justifyContent: "center",
          minWidth: 22,
          height: 22,
          padding: "0 7px",
          borderRadius: 999,
          backgroundColor: C.bgSubtle,
          color: C.textMuted,
          fontSize: MICRO,
          fontWeight: 600,
          fontVariantNumeric: "tabular-nums",
        }}
      >
        {comments.length}
      </span>
    </div>

    <ol style={{ margin: 0, padding: 0, listStyle: "none" }}>
      {comments.map((c, i) => (
        <li
          key={i}
          style={{
            position: "relative",
            display: "grid",
            gridTemplateColumns: "32px 1fr",
            gap: 14, // 0.875rem
            paddingBottom: i === comments.length - 1 ? 0 : 24,
          }}
        >
          {/* connector line (all but last). */}
          {i === comments.length - 1 ? null : (
            <div
              style={{
                position: "absolute",
                left: 15,
                top: 40,
                bottom: 0,
                width: 2,
                borderRadius: 999,
                backgroundColor: C.border,
              }}
            />
          )}
          {/* avatar */}
          <div
            style={{
              width: 32,
              height: 32,
              borderRadius: 999,
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              backgroundColor: C.accentSubtle,
              color: C.accent,
              border: `1px solid ${C.border}`,
              fontSize: MICRO,
              fontWeight: 700,
            }}
          >
            {c.initials}
          </div>
          {/* body */}
          <div style={{ minWidth: 0, paddingTop: 3 }}>
            <div
              style={{
                display: "flex",
                alignItems: "baseline",
                gap: 8,
                marginBottom: 2,
              }}
            >
              <span
                style={{
                  fontSize: BODY_TXT,
                  fontWeight: 600,
                  color: C.text,
                }}
              >
                {c.author}
              </span>
              {c.bot ? <AgentBadge /> : null}
              <span style={{ fontSize: CAPTION, color: C.textMuted }}>
                {c.time}
              </span>
            </div>
            <div
              style={{
                fontSize: BODY_TXT,
                lineHeight: 1.6,
                color: C.text,
              }}
            >
              {c.body}
            </div>
          </div>
        </li>
      ))}
    </ol>
  </section>
);

// ── Whole-app frame: L-chrome + recessed detail content ──────

/**
 * The full app frame at native CSS px, content panel showing the ISSUE
 * DETAIL (the two-column DocumentDetail). Reuses Sidebar (Issues active)
 * from lific-ui. `contentOpacity` lets the scene crossfade the panel in.
 */
export const IssueDetailPage: React.FC<{
  width: number;
  height: number;
  issue: IssueDetailData;
}> = ({ width, height, issue }) => (
  <div
    style={{
      width,
      height,
      display: "flex",
      backgroundColor: C.chrome,
      overflow: "hidden",
      position: "relative",
      fontFamily: BODY,
    }}
  >
    <Sidebar active="issues" />
    <div
      style={{
        flex: 1,
        minWidth: 0,
        display: "flex",
        flexDirection: "column",
      }}
    >
      <DetailTopbar issue={issue} />
      {/* Recessed content panel: rounded-tl-xl + cast shadows. */}
      <div
        style={{
          position: "relative",
          flex: 1,
          minWidth: 0,
          overflow: "hidden",
          borderTopLeftRadius: 12,
          backgroundColor: C.bg,
        }}
      >
        {/* Two-column body: main column + metadata aside, max-w-[1120px]. */}
        <div
          style={{
            position: "absolute",
            inset: 0,
            display: "flex",
            justifyContent: "center",
            overflow: "hidden",
          }}
        >
          <div
            style={{
              width: "100%",
              maxWidth: 1120,
              display: "flex",
              minHeight: "100%",
            }}
          >
            {/* Main column (px-8 py-6). */}
            <div
              style={{
                flex: 1,
                minWidth: 0,
                padding: "24px 32px", // sm:px-8 sm:py-6
                boxSizing: "border-box",
              }}
            >
              {/* InlineTitle (text-title 22px, font-display, mb-4). */}
              <h1
                style={{
                  margin: "0 0 16px",
                  padding: "4px 0",
                  fontFamily: DISPLAY,
                  fontSize: TITLE,
                  fontWeight: 600,
                  letterSpacing: "-0.02em",
                  lineHeight: 1.2,
                  color: C.text,
                }}
              >
                {issue.title}
              </h1>

              {/* Prose description (.prose: 14px / 1.7). */}
              <div
                style={{
                  fontSize: BODY_TXT,
                  lineHeight: 1.7,
                  color: C.text,
                  maxWidth: 680,
                }}
              >
                {issue.description}
              </div>

              <ActivityTimeline items={issue.activity} />
              <CommentThread comments={issue.comments} />
            </div>

            {/* Metadata aside. */}
            <MetaSidebar issue={issue} />
          </div>
        </div>

        {/* Cast shadows (top + left edges of the recessed panel). */}
        <div
          style={{
            pointerEvents: "none",
            position: "absolute",
            top: 0,
            left: 0,
            right: 0,
            height: 24,
            background:
              "linear-gradient(to bottom, rgba(0,0,0,0.17), transparent)",
          }}
        />
        <div
          style={{
            pointerEvents: "none",
            position: "absolute",
            top: 0,
            left: 0,
            bottom: 0,
            width: 24,
            background:
              "linear-gradient(to right, rgba(0,0,0,0.17), transparent)",
          }}
        />
      </div>
    </div>
  </div>
);

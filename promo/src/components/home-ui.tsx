import React from "react";
import { staticFile, Img } from "remotion";
import { C } from "../theme";
import { BODY, DISPLAY, MONO } from "../fonts";
import { ChevronRight, Search, Plus, Circle, CircleDot } from "./icons";
import { PriorityIcon } from "./lific-ui";
import {
  House,
  Sunrise,
  Command,
  Inbox,
  UserCheck,
  User,
  Hourglass,
  Settings,
  Monitor,
  CircleQuestionMark,
  PanelLeftClose,
} from "./lucide-extra";

/*
 * The signed-in Home page at desktop width, dark theme: Layout.svelte's
 * current sidebar and topbar around routes/Home.svelte (greeting, the
 * "Needs you" box, "My active issues"). Sizes are the computed CSS px of
 * the real classes, measured from the running app at an 880px viewport,
 * where the right rail stacks below the fold.
 */

// Type scale (web/src/app.css @theme)
const MICRO = 11;
const CAPTION = 12;
const BODY_SM = 13;

// Sidebar tokens resolved from the app's color-mix() values (dark).
const SIDEBAR_HOVER = "#232926"; // --sidebar-hover
const LAUNCHER_BG = "#141817"; // .sidebar-launcher
const NAV_ACTIVE_BG = "#272c31"; // aria-current destination
const NAV_ACTIVE_FG = "#9d96da"; // its icon, and the avatar initials
const FOOTER_RULE = "#313a35"; // .sidebar-footer border-top
const BTN_SUCCESS = "#3bb266";

/** Layout.svelte initials(): first letter of up to two words. */
export const initials = (name: string): string =>
  name
    .split(/[\s_-]+/)
    .slice(0, 2)
    .map((w) => w[0]?.toUpperCase() ?? "")
    .join("");

export const HOME_SIDEBAR_W = 230;
const TOPBAR_H = 36.8;

// ── Layout.svelte sidebar (desktop) ──────────────────────────

export const AppSidebar: React.FC<{
  height: number;
  version: string;
  projects: { identifier: string; name: string }[];
  user: string;
}> = ({ height, version, projects, user }) => (
  <div
    style={{
      position: "relative",
      width: HOME_SIDEBAR_W,
      height,
      flexShrink: 0,
      backgroundColor: C.chrome,
      fontFamily: BODY,
    }}
  >
    {/* Brand header: px-3 pt-3 pb-2 */}
    <div style={{ display: "flex", alignItems: "center", gap: 6, padding: "12px 12px 8px" }}>
      <div style={{ flex: 1, minWidth: 0, display: "flex", alignItems: "center", gap: 10, padding: 4 }}>
        <Img src={staticFile("logo.webp")} style={{ width: 26, height: 26, borderRadius: 6 }} />
        <span
          style={{
            flex: 1,
            fontFamily: DISPLAY,
            fontSize: 18,
            lineHeight: 1,
            letterSpacing: "-0.025em",
            color: C.text,
          }}
        >
          Lific
        </span>
        <span style={{ fontSize: MICRO, lineHeight: "17.6px", color: C.textFaint }}>v{version}</span>
      </div>
      <div style={{ width: 28, height: 28, display: "grid", placeItems: "center" }}>
        <PanelLeftClose size={15} color={C.textFaint} />
      </div>
    </div>

    {/* Jump to… launcher */}
    <div style={{ padding: "0 12px" }}>
      <div
        style={{
          height: 32,
          boxSizing: "border-box",
          display: "flex",
          alignItems: "center",
          gap: 8,
          padding: "0 10px",
          borderRadius: 6,
          border: `1px solid ${C.border}`,
          backgroundColor: LAUNCHER_BG,
          color: C.textMuted,
        }}
      >
        <Search size={14} color={C.textMuted} />
        <span style={{ flex: 1, fontSize: BODY_SM, lineHeight: "20.8px" }}>Jump to…</span>
        <span
          style={{
            fontFamily: MONO,
            fontSize: MICRO,
            lineHeight: 1,
            color: C.textFaint,
            backgroundColor: SIDEBAR_HOVER,
            borderRadius: 4,
            padding: "2px 4px",
          }}
        >
          ⌘K
        </span>
      </div>
    </div>

    {/* Nav */}
    <div style={{ padding: "16px 8px 4px" }}>
      <div
        style={{
          height: 32.8,
          display: "flex",
          alignItems: "center",
          gap: 8,
          padding: "0 10px",
          borderRadius: 6,
          backgroundColor: NAV_ACTIVE_BG,
          fontSize: BODY_SM,
          fontWeight: 500,
          color: C.text,
        }}
      >
        <House size={14} color={NAV_ACTIVE_FG} />
        Home
      </div>

      <div
        style={{
          marginTop: 15.2,
          height: 32,
          display: "flex",
          alignItems: "center",
          justifyContent: "space-between",
          padding: "0 8px",
        }}
      >
        <span
          style={{
            fontSize: MICRO,
            fontWeight: 600,
            textTransform: "uppercase",
            letterSpacing: "0.08em",
            color: C.textFaint,
          }}
        >
          Projects
        </span>
        <div style={{ width: 32, height: 32, display: "grid", placeItems: "center" }}>
          <Plus size={13} color={C.textFaint} />
        </div>
      </div>

      {projects.map((p) => (
        <div
          key={p.identifier}
          style={{
            marginTop: 8,
            height: 28,
            display: "flex",
            alignItems: "center",
            fontSize: BODY_SM,
            fontWeight: 500,
            color: C.text,
          }}
        >
          <div style={{ width: 28, height: 28, display: "grid", placeItems: "center" }}>
            <ChevronRight size={13} color={C.textFaint} />
          </div>
          <span
            style={{
              width: 20,
              height: 20,
              borderRadius: 4,
              display: "grid",
              placeItems: "center",
              backgroundColor: SIDEBAR_HOVER,
              fontSize: MICRO,
              fontWeight: 500,
              letterSpacing: "-0.025em",
              color: C.textMuted,
            }}
          >
            {p.identifier.slice(0, 2)}
          </span>
          <span style={{ marginLeft: 6, lineHeight: "20.8px" }}>{p.name}</span>
        </div>
      ))}
    </div>

    {/* Footer: account + theme + shortcuts */}
    <div
      style={{
        position: "absolute",
        left: 0,
        right: 0,
        bottom: 0,
        height: 61,
        boxSizing: "border-box",
        borderTop: `1px solid ${FOOTER_RULE}`,
        padding: 8,
        display: "flex",
        alignItems: "center",
        gap: 4,
      }}
    >
      <div style={{ flex: 1, minWidth: 0, display: "flex", alignItems: "center", gap: 8, padding: "6px 8px" }}>
        <div
          style={{
            width: 28,
            height: 28,
            borderRadius: 14,
            display: "grid",
            placeItems: "center",
            backgroundColor: NAV_ACTIVE_BG,
            color: NAV_ACTIVE_FG,
            fontSize: MICRO,
            fontWeight: 600,
            letterSpacing: "0.025em",
          }}
        >
          {initials(user)}
        </div>
        <div style={{ flex: 1, minWidth: 0 }}>
          <div style={{ fontSize: BODY_SM, lineHeight: 1.25, color: C.text }}>{user}</div>
          <div
            style={{
              marginTop: 2,
              display: "flex",
              alignItems: "center",
              gap: 4,
              fontSize: MICRO,
              lineHeight: 1.25,
              color: C.textFaint,
            }}
          >
            <Settings size={9} color={C.textFaint} /> Settings
          </div>
        </div>
      </div>
      <div style={{ width: 28, height: 28, display: "grid", placeItems: "center" }}>
        <Monitor size={15} color={C.textFaint} />
      </div>
      <div style={{ width: 28, height: 28, display: "grid", placeItems: "center" }}>
        <CircleQuestionMark size={15} color={C.textFaint} />
      </div>
    </div>
  </div>
);

// ── Home.svelte pieces ───────────────────────────────────────

export type HomeIssue = {
  identifier: string;
  title: string;
  status: "todo" | "active";
  priority?: "urgent" | "high" | "medium" | "low";
};

const Status: React.FC<{ status: string }> = ({ status }) =>
  status === "active" ? <CircleDot size={14} color={C.accent} /> : <Circle size={14} color={C.textMuted} />;

const SectionHeader: React.FC<{ icon?: React.ReactNode; label: string; count: number }> = ({
  icon,
  label,
  count,
}) => (
  <div style={{ display: "flex", alignItems: "center", gap: 8, height: 17.6, marginBottom: 12 }}>
    {icon}
    <span
      style={{
        fontFamily: DISPLAY,
        fontSize: MICRO,
        lineHeight: "13.2px",
        fontWeight: 600,
        textTransform: "uppercase",
        letterSpacing: "0.1em",
        color: C.textMuted,
      }}
    >
      {label}
    </span>
    <span style={{ fontSize: MICRO, color: C.textFaint, fontVariantNumeric: "tabular-nums" }}>{count}</span>
  </div>
);

const Card: React.FC<{ children: React.ReactNode }> = ({ children }) => (
  <div
    style={{
      borderRadius: 12,
      backgroundColor: C.surface,
      border: `1px solid ${C.border}`,
      overflow: "hidden",
    }}
  >
    {children}
  </div>
);

const IssueLine: React.FC<{ issue: HomeIssue; right: React.ReactNode; rule?: boolean }> = ({
  issue,
  right,
  rule,
}) => (
  <div
    style={{
      display: "flex",
      alignItems: "center",
      gap: 10,
      padding: "8px 16px",
      borderBottom: rule ? `1px solid ${C.border}` : "none",
    }}
  >
    <Status status={issue.status} />
    <span
      style={{
        width: 64,
        flexShrink: 0,
        fontFamily: MONO,
        fontSize: CAPTION,
        lineHeight: "19.2px",
        color: C.textFaint,
        whiteSpace: "nowrap",
      }}
    >
      {issue.identifier}
    </span>
    <span style={{ flex: 1, minWidth: 0, fontSize: BODY_SM, lineHeight: "20.8px", color: C.text }}>
      {issue.title}
    </span>
    {right}
  </div>
);

export type AttentionGroup = {
  key: "assigned" | "human" | "waiting";
  issues: HomeIssue[];
};

const GROUP_META = {
  assigned: { label: "Assigned to you", Icon: UserCheck },
  human: { label: "For any person", Icon: User },
  waiting: { label: "Waiting on you", Icon: Hourglass },
} as const;

/** Home.svelte "Needs you" (LIF-506): empty groups are hidden. */
export const NeedsYou: React.FC<{ groups: AttentionGroup[]; projectName: string }> = ({
  groups,
  projectName,
}) => {
  const shown = groups.filter((g) => g.issues.length > 0);
  const total = new Set(shown.flatMap((g) => g.issues.map((i) => i.identifier))).size;
  return (
    <div style={{ marginBottom: 32 }}>
      <SectionHeader icon={<Inbox size={12} color={C.textFaint} />} label="Needs you" count={total} />
      <Card>
        {shown.map((g, gi) => {
          const { label, Icon } = GROUP_META[g.key];
          return (
            <div
              key={g.key}
              style={{ borderBottom: gi < shown.length - 1 ? `1px solid ${C.border}` : "none" }}
            >
              <div
                style={{
                  display: "flex",
                  alignItems: "center",
                  gap: 8,
                  padding: "10px 16px 6px",
                  fontSize: CAPTION,
                  lineHeight: "19.2px",
                  fontWeight: 500,
                  color: C.textMuted,
                }}
              >
                <Icon size={13} color={C.textFaint} />
                <span>{label}</span>
                <span style={{ fontSize: MICRO, color: C.textFaint, fontVariantNumeric: "tabular-nums" }}>
                  {g.issues.length}
                </span>
              </div>
              {g.issues.map((issue) => (
                <IssueLine
                  key={issue.identifier}
                  issue={issue}
                  right={
                    <span style={{ fontSize: MICRO, lineHeight: "17.6px", color: C.textFaint }}>
                      {projectName}
                    </span>
                  }
                />
              ))}
            </div>
          );
        })}
      </Card>
    </div>
  );
};

/** Home.svelte "My active issues": one card per project. */
export const MyActiveIssues: React.FC<{
  project: { identifier: string; name: string };
  issues: HomeIssue[];
}> = ({ project, issues }) => (
  <div>
    <SectionHeader label="My active issues" count={issues.length} />
    <Card>
      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: 8,
          padding: "10px 16px",
          borderBottom: `1px solid ${C.border}`,
        }}
      >
        <span
          style={{
            width: 20,
            height: 20,
            boxSizing: "border-box",
            borderRadius: 6,
            border: `1px solid ${C.border}`,
            backgroundColor: C.bgSubtle,
            display: "grid",
            placeItems: "center",
            fontSize: MICRO,
            fontWeight: 600,
            color: C.textMuted,
          }}
        >
          {project.identifier.slice(0, 2)}
        </span>
        <span style={{ flex: 1, fontSize: BODY_SM, lineHeight: "20.8px", fontWeight: 500, color: C.text }}>
          {project.name}
        </span>
        <span style={{ fontSize: MICRO, color: C.textFaint, fontVariantNumeric: "tabular-nums" }}>
          {issues.length}
        </span>
      </div>
      {issues.map((issue, i) => (
        <IssueLine
          key={issue.identifier}
          issue={issue}
          rule={i < issues.length - 1}
          right={issue.priority ? <PriorityIcon priority={issue.priority} size={15} /> : null}
        />
      ))}
    </Card>
  </div>
);

/** Greeting row: Home.svelte greetingText()/greetingIcon() for 5 to 11 am. */
export const Greeting: React.FC<{ name: string; dateLabel: string }> = ({ name, dateLabel }) => (
  <div
    style={{
      display: "flex",
      alignItems: "flex-start",
      justifyContent: "space-between",
      gap: 16,
      marginBottom: 32,
    }}
  >
    <div style={{ display: "flex", alignItems: "center", gap: 12 }}>
      <span
        style={{
          width: 44,
          height: 44,
          borderRadius: 12,
          backgroundColor: C.accentSubtle,
          display: "grid",
          placeItems: "center",
        }}
      >
        <Sunrise size={20} color={C.accent} />
      </span>
      <div>
        <div
          style={{
            fontFamily: DISPLAY,
            fontSize: 22,
            lineHeight: 1,
            fontWeight: 600,
            letterSpacing: "-0.025em",
            color: C.text,
          }}
        >
          Good morning, {name}
        </div>
        <div style={{ marginTop: 4, fontSize: BODY_SM, lineHeight: "20.8px", color: C.textMuted }}>
          {dateLabel}
        </div>
      </div>
    </div>
    <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: 6,
          padding: "6px 12px",
          borderRadius: 6,
          backgroundColor: BTN_SUCCESS,
          color: C.stone950,
          fontSize: BODY_SM,
          lineHeight: "20.8px",
          fontWeight: 500,
        }}
      >
        <Plus size={14} color={C.stone950} /> New issue
      </div>
      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: 6,
          padding: "6px 12px",
          borderRadius: 6,
          border: `1px solid ${C.border}`,
          color: C.textMuted,
          fontSize: BODY_SM,
          lineHeight: "20.8px",
        }}
      >
        <Command size={13} color={C.textMuted} />
        Jump to…
        <span
          style={{
            marginLeft: 2,
            fontFamily: MONO,
            fontSize: MICRO,
            lineHeight: 1,
            color: C.textFaint,
            border: `1px solid ${C.border}`,
            borderRadius: 4,
            padding: "2px 4px",
          }}
        >
          ⌘K
        </span>
      </div>
    </div>
  </div>
);

/**
 * The whole signed-in app on Home: L-shaped chrome (sidebar + "Home"
 * topbar) around the recessed content panel with its cast shadows.
 */
export const HomeApp: React.FC<{
  width: number;
  height: number;
  version: string;
  user: string;
  dateLabel: string;
  project: { identifier: string; name: string };
  attention: AttentionGroup[];
  active: HomeIssue[];
}> = ({ width, height, version, user, dateLabel, project, attention, active }) => (
  <div
    style={{
      width,
      height,
      display: "flex",
      overflow: "hidden",
      backgroundColor: C.chrome,
      fontFamily: BODY,
      color: C.text,
    }}
  >
    <AppSidebar height={height} version={version} projects={[project]} user={user} />
    <div style={{ flex: 1, minWidth: 0, display: "flex", flexDirection: "column" }}>
      <div style={{ height: TOPBAR_H, display: "flex", alignItems: "center", padding: "0 24px", flexShrink: 0 }}>
        <span style={{ fontSize: BODY_SM, lineHeight: "20.8px", fontWeight: 500, color: C.text }}>Home</span>
      </div>
      <div
        style={{
          position: "relative",
          flex: 1,
          overflow: "hidden",
          borderTopLeftRadius: 12,
          backgroundColor: C.bg,
        }}
      >
        <div style={{ padding: "40px 32px" }}>
          <Greeting name={user} dateLabel={dateLabel} />
          <NeedsYou groups={attention} projectName={project.name} />
          <MyActiveIssues project={project} issues={active} />
        </div>
        <div
          style={{
            position: "absolute",
            top: 0,
            left: 0,
            right: 0,
            height: 24,
            background: "linear-gradient(to bottom, rgba(0,0,0,0.17), transparent)",
          }}
        />
        <div
          style={{
            position: "absolute",
            top: 0,
            left: 0,
            bottom: 0,
            width: 24,
            background: "linear-gradient(to right, rgba(0,0,0,0.17), transparent)",
          }}
        />
      </div>
    </div>
  </div>
);

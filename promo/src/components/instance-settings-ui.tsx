import React from "react";
import { staticFile, Img } from "remotion";
import { C } from "../theme";
import { BODY, DISPLAY, MONO } from "../fonts";
import {
  ChevronRight,
  Search,
  Plus,
  ListIcon,
  LayoutGrid,
  LayoutDashboard,
  Layers,
  FileText,
  ListChecks,
  History,
  TrendingUp,
  Home,
  Moon,
  HelpCircle,
  SettingsGear,
} from "./icons";

/*
 * Pixel-faithful replica of web/src/routes/InstanceSettings.svelte — the
 * admin Instance Settings page — plus the two toggle controls the Teams
 * scene flips (Sign-ups and Project permissions). Every size below is the
 * computed CSS px of the corresponding Tailwind class in the app; copy is
 * lifted verbatim from the Svelte source.
 *
 * Reuses the Layout.svelte sidebar shape (kept local so this file is
 * self-contained and matches the board scene's conventions).
 */

// Type scale (app.css @theme)
const MICRO = 11; // text-micro
const CAPTION = 12; // text-caption
const BODY_SM = 13; // text-body-sm
const BODY_TEXT = 14; // text-body
const BODY_LG = 15; // text-body-lg
const HEADING = 18; // text-heading (wordmark)
const TITLE = 22; // text-title (page h1)

// Dark-mode tokens not present in C (from app.css .dark block).
const SUCCESS_BG = "#142a1b"; // --success-bg (green-bg-dark)
// --warn-text mirrors --warn in dark mode.
const WARN_TEXT = C.warn;
// color-mix(in oklab, --warn 15%, --bg): warn (#fb923c) 15% over bg (#0d1110).
const WARN_FILL_15 = "#2d211a";
// color-mix(in oklab, --warn 12%, --bg): warn 12% over bg (warning box bg).
const WARN_FILL_12 = "#281e1a";
// color-mix(in oklab, --warn 38%, transparent): ring on active amber toggle.
const WARN_RING = "rgba(251,146,60,0.38)";
// color-mix(in oklab, --success 38%, transparent): ring on active green toggle.
const SUCCESS_RING = "rgba(74,222,128,0.38)";

// ── Icons used only on the settings page ─────────────────────

type P = { size: number; color: string };

const IconBase: React.FC<P & { children: React.ReactNode }> = ({
  size,
  color,
  children,
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
    {children}
  </svg>
);

export const SlidersHorizontal: React.FC<P> = (p) => (
  <IconBase {...p}>
    <line x1="21" x2="14" y1="4" y2="4" />
    <line x1="10" x2="3" y1="4" y2="4" />
    <line x1="21" x2="12" y1="12" y2="12" />
    <line x1="8" x2="3" y1="12" y2="12" />
    <line x1="21" x2="16" y1="20" y2="20" />
    <line x1="12" x2="3" y1="20" y2="20" />
    <line x1="14" x2="14" y1="2" y2="6" />
    <line x1="8" x2="8" y1="10" y2="14" />
    <line x1="16" x2="16" y1="18" y2="22" />
  </IconBase>
);

export const DoorOpen: React.FC<P> = (p) => (
  <IconBase {...p}>
    <path d="M13 4h3a2 2 0 0 1 2 2v14" />
    <path d="M2 20h3" />
    <path d="M13 20h9" />
    <path d="M10 12v.01" />
    <path d="M13 4.562v16.157a1 1 0 0 1-1.242.97L5 20V5.562a2 2 0 0 1 1.515-1.94l4-1A2 2 0 0 1 13 4.562" />
  </IconBase>
);

export const DoorClosed: React.FC<P> = (p) => (
  <IconBase {...p}>
    <path d="M18 20V6a2 2 0 0 0-2-2H8a2 2 0 0 0-2 2v14" />
    <path d="M2 20h20" />
    <path d="M14 12v.01" />
  </IconBase>
);

export const Users: React.FC<P> = (p) => (
  <IconBase {...p}>
    <path d="M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2" />
    <circle cx="9" cy="7" r="4" />
    <path d="M22 21v-2a4 4 0 0 0-3-3.87" />
    <path d="M16 3.13a4 4 0 0 1 0 7.75" />
  </IconBase>
);

export const AlertTriangle: React.FC<P> = (p) => (
  <IconBase {...p}>
    <path d="m21.73 18-8-14a2 2 0 0 0-3.48 0l-8 14A2 2 0 0 0 4 21h16a2 2 0 0 0 1.73-3" />
    <path d="M12 9v4" />
    <path d="M12 17h.01" />
  </IconBase>
);

export const Check: React.FC<P> = (p) => (
  <IconBase {...p}>
    <path d="M20 6 9 17l-5-5" />
  </IconBase>
);

export const ShieldCheck: React.FC<P> = (p) => (
  <IconBase {...p}>
    <path d="M20 13c0 5-3.5 7.5-7.66 8.95a1 1 0 0 1-.67-.01C7.5 20.5 4 18 4 13V6a1 1 0 0 1 1-1c2 0 4.5-1.2 6.24-2.72a1.17 1.17 0 0 1 1.52 0C14.51 3.81 17 5 19 5a1 1 0 0 1 1 1z" />
    <path d="m9 12 2 2 4-4" />
  </IconBase>
);

// ── Layout.svelte sidebar (settings flavor: Settings active) ─

const SIDEBAR_W = 230;

const NavItem: React.FC<{
  icon: React.ReactNode;
  label: string;
  active?: boolean;
}> = ({ icon, label, active }) => (
  <div
    style={{
      display: "flex",
      alignItems: "center",
      gap: 8,
      padding: "4px 8px",
      borderRadius: 6,
      fontSize: BODY_SM,
      fontWeight: active ? 500 : 400,
      color: active ? C.text : C.textMuted,
      backgroundColor: active ? C.bgSubtle : "transparent",
    }}
  >
    {icon}
    {label}
  </div>
);

const Sidebar: React.FC = () => {
  const sub = (
    Icon: React.FC<{ size: number; color: string }>,
    label: string,
    active = false,
  ) => (
    <NavItem
      key={label}
      icon={<Icon size={14} color={active ? C.accent : C.textMuted} />}
      label={label}
      active={active}
    />
  );

  return (
    <div
      style={{
        width: SIDEBAR_W,
        flexShrink: 0,
        display: "flex",
        flexDirection: "column",
        backgroundColor: C.chrome,
        fontFamily: BODY,
        boxSizing: "border-box",
      }}
    >
      {/* Brand header */}
      <div style={{ padding: "12px 12px 8px", display: "flex", alignItems: "center", gap: 10 }}>
        <div style={{ display: "flex", flex: 1, alignItems: "center", gap: 10, padding: "4px 4px" }}>
          <Img src={staticFile("logo.webp")} style={{ width: 26, height: 26, borderRadius: 6 }} />
          <span
            style={{
              fontFamily: DISPLAY,
              fontSize: HEADING,
              letterSpacing: "-0.02em",
              color: C.text,
              lineHeight: 1,
              flex: 1,
              fontWeight: 600,
            }}
          >
            Lific
          </span>
          <span
            style={{
              fontFamily: MONO,
              fontSize: MICRO,
              letterSpacing: "-0.02em",
              color: C.textFaint,
              padding: "2px 6px",
              borderRadius: 6,
              backgroundColor: C.bgSubtle,
            }}
          >
            v2.0.0
          </span>
        </div>
      </div>

      {/* Jump to… */}
      <div style={{ padding: "0 12px 8px" }}>
        <div
          style={{
            height: 32,
            display: "flex",
            alignItems: "center",
            gap: 8,
            padding: "0 10px",
            borderRadius: 6,
            backgroundColor: C.bg,
            boxShadow: "inset 0 1px 2px rgba(0,0,0,0.08)",
            color: C.textMuted,
          }}
        >
          <Search size={14} color={C.textMuted} />
          <span style={{ flex: 1, fontSize: BODY_SM }}>Jump to…</span>
          <span
            style={{
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

      {/* Nav */}
      <div style={{ flex: 1, padding: "4px 8px" }}>
        <div
          style={{
            marginBottom: 4,
            padding: "6px 10px",
            display: "flex",
            alignItems: "center",
            gap: 8,
            borderRadius: 6,
            fontSize: BODY_SM,
            color: C.textMuted,
          }}
        >
          <Home size={14} color={C.textMuted} />
          Home
        </div>

        <div
          style={{
            display: "flex",
            alignItems: "center",
            justifyContent: "space-between",
            padding: "6px 8px 4px",
          }}
        >
          <span
            style={{
              fontSize: MICRO,
              fontWeight: 600,
              textTransform: "uppercase",
              letterSpacing: "0.1em",
              color: C.textFaint,
            }}
          >
            Projects
          </span>
          <Plus size={13} color={C.textFaint} />
        </div>

        {/* Active project pill */}
        <div
          style={{
            display: "flex",
            alignItems: "center",
            gap: 6,
            padding: "6px 8px 6px 6px",
            borderRadius: 6,
            fontSize: BODY_SM,
            fontWeight: 500,
            color: C.text,
            backgroundColor: C.bgSubtle,
          }}
        >
          <ChevronRight size={13} color={C.textMuted} rotated />
          <span
            style={{
              width: 20,
              height: 20,
              borderRadius: 6,
              border: `1px solid ${C.border}`,
              backgroundColor: C.bgSubtle,
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              fontSize: MICRO,
              fontWeight: 600,
              letterSpacing: "-0.02em",
              color: C.text,
            }}
          >
            LI
          </span>
          <span style={{ flex: 1 }}>Lific</span>
        </div>

        {/* Sub-nav with tree guide line */}
        <div
          style={{
            marginLeft: 18,
            paddingLeft: 10,
            marginTop: 2,
            marginBottom: 6,
            borderLeft: `1px solid ${C.border}`,
            display: "flex",
            flexDirection: "column",
            gap: 1,
          }}
        >
          {sub(LayoutDashboard, "Overview")}
          {sub(ListIcon, "Issues")}
          {sub(LayoutGrid, "Board")}
          {sub(Layers, "Modules")}
          {sub(FileText, "Pages")}
          {sub(ListChecks, "Plans")}
          {sub(History, "Activity")}
          {sub(TrendingUp, "Insights")}
        </div>
      </div>

      {/* Footer — Settings active */}
      <div style={{ padding: 8, display: "flex", alignItems: "center", gap: 4 }}>
        <div
          style={{
            flex: 1,
            display: "flex",
            alignItems: "center",
            gap: 10,
            padding: "6px 8px",
            borderRadius: 6,
            backgroundColor: C.bgSubtle,
          }}
        >
          <div
            style={{
              width: 28,
              height: 28,
              borderRadius: 14,
              backgroundColor: C.accent,
              color: C.stone950,
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              fontSize: MICRO,
              fontWeight: 600,
              letterSpacing: "0.02em",
            }}
          >
            L
          </div>
          <div style={{ flex: 1, minWidth: 0 }}>
            <div style={{ fontSize: BODY_SM, color: C.text, lineHeight: 1.25 }}>Lizzy</div>
            <div
              style={{
                fontSize: MICRO,
                color: C.accent,
                display: "flex",
                alignItems: "center",
                gap: 4,
                marginTop: 2,
                lineHeight: 1.25,
              }}
            >
              <SettingsGear size={9} color={C.accent} /> Settings
            </div>
          </div>
        </div>
        <div style={{ width: 32, height: 32, display: "grid", placeItems: "center" }}>
          <Moon size={15} color={C.textMuted} />
        </div>
        <div style={{ width: 32, height: 32, display: "grid", placeItems: "center" }}>
          <HelpCircle size={15} color={C.textMuted} />
        </div>
      </div>
    </div>
  );
};

// ── Settings topbar ("Settings") ─────────────────────────────

const SettingsTopbar: React.FC = () => (
  <div
    style={{
      height: 44,
      display: "flex",
      alignItems: "center",
      gap: 12,
      padding: "8px 24px",
      boxSizing: "border-box",
      backgroundColor: C.chrome,
      fontFamily: BODY,
    }}
  >
    <span style={{ fontSize: BODY_SM, fontWeight: 500, color: C.text }}>Settings</span>
  </div>
);

// ── Field label (block micro uppercase) ──────────────────────

const FieldLabel: React.FC<{ children: React.ReactNode; icon?: React.ReactNode }> = ({
  children,
  icon,
}) => (
  <span
    style={{
      display: "flex",
      alignItems: "center",
      gap: 6,
      fontSize: MICRO,
      fontWeight: 600,
      textTransform: "uppercase",
      letterSpacing: "0.1em",
      color: C.text,
      marginBottom: 6,
    }}
  >
    {icon}
    {children}
  </span>
);

const HelpText: React.FC<{ children: React.ReactNode; marginTop?: number }> = ({
  children,
  marginTop = 6,
}) => (
  <span
    style={{
      display: "block",
      fontSize: CAPTION,
      color: C.text,
      marginTop,
      lineHeight: 1.6,
      maxWidth: "42ch",
    }}
  >
    {children}
  </span>
);

const TextInput: React.FC<{
  value?: string;
  placeholder?: string;
  mono?: boolean;
  width?: number | string;
}> = ({ value, placeholder, mono, width = "100%" }) => (
  <div
    style={{
      width,
      boxSizing: "border-box",
      padding: "8px 12px",
      fontSize: BODY_TEXT,
      fontFamily: mono ? MONO : BODY,
      borderRadius: 6,
      border: `1px solid ${C.border}`,
      backgroundColor: C.bg,
      color: value ? C.text : C.textFaint,
    }}
  >
    {value || placeholder}
  </div>
);

// ── Segmented toggle (Sign-ups / Project permissions shape) ──

export type SegSide = {
  icon: React.ReactNode;
  label: string;
  tone: "neutral" | "success" | "warn";
};

/**
 * The inline-flex segmented control used by Sign-ups, Single-user mode and
 * Project permissions. `activeProgress` (0..1) drives which side reads as
 * active during the scripted flip — 0 = left active, 1 = right active. The
 * inactive slot fades to the muted style; the active slot fades in its tinted
 * fill + ring, so the flip is a pure function of the scene frame.
 */
export const SegmentedToggle: React.FC<{
  left: SegSide;
  right: SegSide;
  activeProgress: number; // 0 => left active, 1 => right active
}> = ({ left, right, activeProgress }) => {
  const t = Math.max(0, Math.min(1, activeProgress));
  return (
    <div
      style={{
        display: "inline-flex",
        gap: 4,
        padding: 4,
        borderRadius: 12,
        backgroundColor: C.bg,
        boxShadow: "inset 0 1px 2px rgba(0,0,0,0.10)",
      }}
    >
      <SegButton side={left} active={1 - t} />
      <SegButton side={right} active={t} />
    </div>
  );
};

const toneStyle = (tone: SegSide["tone"]) => {
  if (tone === "success")
    return { bg: SUCCESS_BG, color: C.success, ring: SUCCESS_RING };
  if (tone === "warn") return { bg: WARN_FILL_15, color: WARN_TEXT, ring: WARN_RING };
  // neutral active = surface fill + border ring
  return { bg: C.surface, color: C.text, ring: C.border };
};

const SegButton: React.FC<{ side: SegSide; active: number }> = ({ side, active }) => {
  const a = Math.max(0, Math.min(1, active));
  const ts = toneStyle(side.tone);
  // Blend inactive (muted, transparent) -> active (tinted fill + ring).
  const color = a > 0.5 ? ts.color : C.textMuted;
  return (
    <div
      style={{
        display: "flex",
        alignItems: "center",
        gap: 8,
        padding: "8px 16px",
        borderRadius: 8,
        fontSize: BODY_SM,
        fontWeight: 600,
        color,
        backgroundColor: `rgba(0,0,0,0)`,
        position: "relative",
        boxSizing: "border-box",
      }}
    >
      {/* active fill layer (opacity animates) */}
      <div
        style={{
          position: "absolute",
          inset: 0,
          borderRadius: 8,
          backgroundColor: ts.bg,
          boxShadow: `0 1px 2px rgba(0,0,0,0.10)`,
          border: `1px solid ${ts.ring}`,
          opacity: a,
        }}
      />
      <span style={{ position: "relative", display: "flex", alignItems: "center", gap: 8 }}>
        {React.cloneElement(side.icon as React.ReactElement<{ color: string }>, { color })}
        {side.label}
      </span>
    </div>
  );
};

const WarnBox: React.FC<{ children: React.ReactNode; opacity: number }> = ({
  children,
  opacity,
}) => (
  <div
    style={{
      display: "flex",
      alignItems: "flex-start",
      gap: 8,
      fontSize: CAPTION,
      color: WARN_TEXT,
      backgroundColor: WARN_FILL_12,
      padding: "8px 12px",
      borderRadius: 8,
      marginTop: 8,
      maxWidth: "42ch",
      lineHeight: 1.5,
      opacity,
    }}
  >
    <div style={{ marginTop: 2 }}>
      <AlertTriangle size={13} color={WARN_TEXT} />
    </div>
    <span>{children}</span>
  </div>
);

// ── Instance Settings page ───────────────────────────────────

/**
 * The scrollable settings page content. Progress values (0..1) drive the two
 * scripted flips; the caller animates them from the scene frame. The rest is
 * static — faithful to what's visible on the real page for an admin.
 */
export type InstanceSettingsProps = {
  width: number;
  height: number;
  host: string;
  signupsProgress: number; // 0 = Closed active, 1 = Open active
  authzProgress: number; // 0 = Off active, 1 = Enforced active
  scrollY?: number; // px the content column is scrolled up
};

export const InstanceSettingsPage: React.FC<InstanceSettingsProps> = ({
  width,
  height,
  host,
  signupsProgress,
  authzProgress,
  scrollY = 0,
}) => {
  const signupsOpen = signupsProgress > 0.5;
  const authzEnforced = authzProgress > 0.5;

  return (
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
      <Sidebar />
      <div style={{ flex: 1, minWidth: 0, display: "flex", flexDirection: "column" }}>
        <SettingsTopbar />
        {/* Recessed content panel */}
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
          {/* Content column: max-w-[1000px] mx-auto px-6 py-10 */}
          <div
            style={{
              maxWidth: 1000,
              margin: "0 auto",
              padding: "40px 24px",
              boxSizing: "border-box",
              transform: `translateY(${-scrollY}px)`,
            }}
          >
            {/* Settings tabs (Account / Instance) */}
            <div
              style={{
                display: "flex",
                alignItems: "center",
                gap: 24,
                borderBottom: `1px solid ${C.border}`,
                marginBottom: 32,
              }}
            >
              <div style={{ padding: "4px 2px 10px", fontSize: BODY_TEXT, fontWeight: 500, color: C.textMuted }}>
                Account
              </div>
              <div
                style={{
                  padding: "4px 2px 10px",
                  marginBottom: -1,
                  fontSize: BODY_TEXT,
                  fontWeight: 500,
                  color: C.text,
                  borderBottom: `2px solid ${C.accent}`,
                }}
              >
                Instance
              </div>
            </div>

            {/* Page heading */}
            <div style={{ marginBottom: 32 }}>
              <h1
                style={{
                  margin: 0,
                  fontFamily: DISPLAY,
                  fontSize: TITLE,
                  fontWeight: 600,
                  letterSpacing: "-0.02em",
                  color: C.text,
                  lineHeight: 1,
                }}
              >
                Instance
              </h1>
              <p style={{ margin: "8px 0 0", fontSize: BODY_TEXT, color: C.textMuted, lineHeight: 1.6 }}>
                Settings for the Lific instance at{" "}
                <span style={{ fontFamily: MONO, color: C.text }}>{host}</span>. Changes apply
                immediately.
              </p>
            </div>

            {/* Settings card */}
            <div
              style={{
                borderRadius: 12,
                backgroundColor: C.surface,
                boxShadow: "0 1px 2px rgba(0,0,0,0.06)",
                padding: 20,
              }}
            >
              <div style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: 20 }}>
                <SlidersHorizontal size={15} color={C.textMuted} />
                <h2 style={{ margin: 0, fontSize: BODY_LG, fontWeight: 600, color: C.text }}>Settings</h2>
                <span
                  style={{
                    fontFamily: MONO,
                    fontSize: MICRO,
                    color: C.textFaint,
                    padding: "2px 6px",
                    borderRadius: 4,
                    backgroundColor: C.bgSubtle,
                  }}
                >
                  v2.0.0
                </span>
              </div>

              <div style={{ display: "flex", flexDirection: "column", gap: 24, maxWidth: 560 }}>
                {/* Instance name */}
                <div>
                  <FieldLabel>Instance name</FieldLabel>
                  <TextInput value="Lific" placeholder={host} />
                  <HelpText>Shown on the sign-in screen. Leave blank to use the host.</HelpText>
                </div>

                {/* Sign-ups — SCRIPTED FLIP #1 */}
                <div>
                  <FieldLabel>Sign-ups</FieldLabel>
                  <SegmentedToggle
                    left={{ icon: <DoorOpen size={16} color={C.textMuted} />, label: "Open", tone: "success" }}
                    right={{ icon: <DoorClosed size={16} color={C.textMuted} />, label: "Closed", tone: "warn" }}
                    activeProgress={1 - signupsProgress}
                  />
                  <HelpText marginTop={8}>
                    {signupsOpen
                      ? "Anyone can create their own account."
                      : "New accounts are created by an admin only. The sign-in screen shows a closed notice."}
                  </HelpText>
                </div>

                {/* Allowed signup domains */}
                <div>
                  <FieldLabel>Allowed signup domains</FieldLabel>
                  <TextInput placeholder="snake.com, sub.snake.com" mono />
                  <HelpText>Comma-separated. Leave blank to allow any email domain.</HelpText>
                </div>

                {/* Session lifetime */}
                <div>
                  <FieldLabel>Session lifetime</FieldLabel>
                  <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
                    <TextInput value="30" width={96} />
                    <span style={{ fontSize: BODY_TEXT, color: C.text }}>days</span>
                  </div>
                  <HelpText>How long a sign-in stays valid before re-authenticating (1 to 365).</HelpText>
                </div>

                {/* Project permissions — SCRIPTED FLIP #2 (divider above) */}
                <div style={{ paddingTop: 24, marginTop: 4, borderTop: `1px solid ${C.border}` }}>
                  <FieldLabel icon={<Users size={12} color={C.text} />}>Project permissions</FieldLabel>
                  <SegmentedToggle
                    left={{ icon: <DoorOpen size={16} color={C.textMuted} />, label: "Off", tone: "neutral" }}
                    right={{ icon: <DoorClosed size={16} color={C.textMuted} />, label: "Enforced", tone: "warn" }}
                    activeProgress={authzProgress}
                  />
                  <HelpText marginTop={8}>
                    When on, only project members can see or edit a project. Add yourself as lead to
                    your projects before enabling.
                  </HelpText>
                  {authzEnforced ? (
                    <WarnBox opacity={Math.max(0, Math.min(1, (authzProgress - 0.5) * 2))}>
                      Anyone not added as a project member (via that project's Settings → Members)
                      loses access to it immediately, including you if you aren't a lead yet.
                    </WarnBox>
                  ) : null}
                </div>
              </div>

              {/* Autosave status */}
              <div style={{ display: "flex", alignItems: "center", gap: 8, marginTop: 20, height: 20, fontSize: BODY_SM }}>
                <span style={{ color: C.textMuted }}>Changes save automatically.</span>
              </div>
            </div>

            {/* Members section header (visible below the card) */}
            <div style={{ marginTop: 40 }}>
              <div style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: 4 }}>
                <ShieldCheck size={16} color={C.textMuted} />
                <h2 style={{ margin: 0, fontSize: 16, fontWeight: 600, color: C.text }}>Members</h2>
              </div>
              <p style={{ margin: 0, fontSize: BODY_TEXT, color: C.textMuted, lineHeight: 1.6 }}>
                3 people on this instance · 1 admin.
              </p>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
};

export const SETTINGS_SIDEBAR_W = SIDEBAR_W;

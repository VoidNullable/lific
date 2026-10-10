import React from "react";
import { AbsoluteFill } from "remotion";
import { C } from "./theme";
import { BODY } from "./fonts";
import { HomeApp, type AttentionGroup, type HomeIssue } from "./components/home-ui";
import { MobileIssueView } from "./components/mobile-issue-ui";

/*
 * HumansStill: the landing page's "For humans" pair. Left, Blake's Home
 * page on Thursday morning with the "Needs you" box (dark theme, an 880px
 * browser viewport). Right, the same morning on his phone: APP-51, where
 * the agent asked him to pick a launch date with a task list and he just
 * ticked one.
 *
 * The background is transparent so the phone keeps its rounded corners
 * when it overlaps the browser window on the page. The site's Window
 * draws the browser chrome, so the Home pane has none.
 *
 *   bunx remotion still HumansStill /tmp/opencode/humans-still.png
 *   magick /tmp/opencode/humans-still.png -crop 1760x1440+0+0 +repage -quality 90 ../site/public/humans-home.webp
 *   magick /tmp/opencode/humans-still.png -crop 660x1386+1820+27 +repage -quality 90 ../site/public/humans-phone.webp
 */

// ── Geometry ─────────────────────────────────────────────────
const HOME_W = 880; // CSS px viewport, below the lg breakpoint
const HOME_H = 720;
const S_HOME = 2;

const SCREEN_W = 390;
const SCREEN_H = 844;
const BEZEL = 11;
const INSET = 36; // safe-area-inset-top: the status bar
const S_PHONE = 1.6;
const PHONE_W = SCREEN_W + BEZEL * 2; // 412
const PHONE_H = SCREEN_H + BEZEL * 2; // 866

const GAP = 60;
const PHONE_X = HOME_W * S_HOME + GAP; // 1820
const PHONE_Y = Math.round((HOME_H * S_HOME - PHONE_H * S_PHONE) / 2); // 27

export const HUMANS_STILL_W = PHONE_X + Math.ceil(PHONE_W * S_PHONE); // 2480
export const HUMANS_STILL_H = HOME_H * S_HOME; // 1440

// ── The morning's data ───────────────────────────────────────
const PROJECT = { identifier: "APP", name: "App" };

const APP43: HomeIssue = { identifier: "APP-43", title: "Retry with exponential backoff", status: "active", priority: "high" };
const APP44: HomeIssue = { identifier: "APP-44", title: "Approve retry limits", status: "todo", priority: "medium" };
const APP48: HomeIssue = { identifier: "APP-48", title: "Sync status in the header", status: "active", priority: "medium" };
const APP51: HomeIssue = { identifier: "APP-51", title: "Pick the launch date", status: "todo", priority: "medium" };
const APP52: HomeIssue = { identifier: "APP-52", title: "Sign the vendor contract", status: "todo", priority: "low" };

const ATTENTION: AttentionGroup[] = [
  { key: "assigned", issues: [APP51] },
  { key: "human", issues: [APP52] },
  { key: "waiting", issues: [APP44] },
];

// Home.svelte compareIssues: active before todo, then priority.
const ACTIVE = [APP43, APP48, APP44, APP51, APP52];

// ── A plain phone: rounded body, punch-hole camera, no marks ─
const SignalBars: React.FC = () => (
  <svg width={17} height={11} viewBox="0 0 17 11" style={{ display: "block" }}>
    {[0, 1, 2, 3].map((i) => (
      <rect key={i} x={i * 4.5} y={8 - i * 2.6} width={3} height={3 + i * 2.6} rx={0.8} fill={C.text} />
    ))}
  </svg>
);

const Battery: React.FC = () => (
  <svg width={25} height={12} viewBox="0 0 25 12" style={{ display: "block" }}>
    <rect x={0.5} y={0.5} width={21} height={11} rx={3} fill="none" stroke={C.text} strokeOpacity={0.45} />
    <rect x={2} y={2} width={15} height={8} rx={1.6} fill={C.text} />
    <rect x={22.8} y={4} width={1.6} height={4} rx={0.8} fill={C.text} fillOpacity={0.45} />
  </svg>
);

const Phone: React.FC<{ children: React.ReactNode }> = ({ children }) => (
  <div
    style={{
      position: "relative",
      width: PHONE_W,
      height: PHONE_H,
      boxSizing: "border-box",
      borderRadius: 52,
      padding: BEZEL,
      backgroundColor: "#0a0c0b",
      boxShadow: "inset 0 0 0 1.5px #3a433e, inset 0 0 0 3px #060706",
    }}
  >
    <div
      style={{
        position: "relative",
        width: SCREEN_W,
        height: SCREEN_H,
        borderRadius: 42,
        overflow: "hidden",
        backgroundColor: C.bg,
      }}
    >
      {children}
      {/* Status bar over the app's chrome header */}
      <div
        style={{
          position: "absolute",
          left: 0,
          top: 0,
          width: SCREEN_W,
          height: INSET,
          display: "flex",
          alignItems: "center",
          justifyContent: "space-between",
          padding: "0 30px 0 34px",
          boxSizing: "border-box",
          fontFamily: BODY,
          color: C.text,
        }}
      >
        <span style={{ fontSize: 14, fontWeight: 600, fontVariantNumeric: "tabular-nums" }}>9:06</span>
        <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
          <SignalBars />
          <Battery />
        </div>
      </div>
      <div
        style={{
          position: "absolute",
          left: SCREEN_W / 2 - 6,
          top: 12,
          width: 12,
          height: 12,
          borderRadius: 6,
          backgroundColor: "#030404",
          boxShadow: "0 0 0 1px #1a1f1d",
        }}
      />
    </div>
  </div>
);

export const HumansStill: React.FC = () => (
  <AbsoluteFill>
    <div style={{ position: "absolute", left: 0, top: 0, transform: `scale(${S_HOME})`, transformOrigin: "top left" }}>
      <HomeApp
        width={HOME_W}
        height={HOME_H}
        version="2.10.0"
        user="Blake"
        dateLabel="Thursday, October 8"
        project={PROJECT}
        attention={ATTENTION}
        active={ACTIVE}
      />
    </div>

    <div
      style={{
        position: "absolute",
        left: PHONE_X,
        top: PHONE_Y,
        transform: `scale(${S_PHONE})`,
        transformOrigin: "top left",
      }}
    >
      <Phone>
        <MobileIssueView
          height={SCREEN_H}
          inset={INSET}
          project={PROJECT}
          identifier="APP-51"
          status="todo"
          title="Pick the launch date"
          tasks={[
            { text: "Oct 20 (after the beta)", done: false },
            { text: "Oct 27 (gives docs a week)", done: true },
            { text: "Nov 3", done: false },
          ]}
          timeline={[
            { actor: "Blake", verb: "changed description", showChange: true, when: "just now", via: "web" },
            { actor: "opencode-blake", agent: true, verb: "assigned", object: "@blake", when: "2h ago", via: "mcp" },
            { actor: "opencode-blake", agent: true, verb: "created this issue", when: "2h ago", via: "mcp" },
          ]}
        />
      </Phone>
    </div>
  </AbsoluteFill>
);

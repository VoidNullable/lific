import React from "react";
import { AbsoluteFill, staticFile, Img } from "remotion";
import { C } from "./theme";
import { DISPLAY } from "./fonts";
import {
  LificApp,
  IssueCard,
  IssueData,
  Label,
  colX,
  cardsTop,
  CARD_W,
  CARD_PAD,
} from "./components/lific-ui";

/*
 * README hero image (replaces the old Photoshop LificHero.png): the
 * pixel-faithful board replica dimmed behind the current logo + Space
 * Grotesk wordmark. Rendered as a Still — regenerate any time with:
 *   npx remotion still Hero ../LificHero.png
 * Everything is static (no useCurrentFrame), so frame 0 is the image.
 */

const L: Record<string, Label> = {
  webui: { name: "web-ui", color: "#4dd9c7" },
  core: { name: "core", color: "#9287d7" },
  mcp: { name: "mcp", color: "#b48af0" },
  auth: { name: "auth", color: "#fb923c" },
  bug: { name: "bug", color: "#f87171" },
};

const CARDS: { issue: IssueData; col: number; slot: number }[] = [
  {
    col: 0,
    slot: 0,
    issue: {
      identifier: "LIF-231",
      title: "Board column virtualization",
      priority: "medium",
      labels: [L.webui],
      updated: "2d ago",
    },
  },
  {
    col: 0,
    slot: 1,
    issue: {
      identifier: "LIF-214",
      title: "Bulk-edit issues from the list",
      priority: "high",
      labels: [L.webui],
      updated: "4h ago",
    },
  },
  {
    col: 0,
    slot: 2,
    issue: {
      identifier: "LIF-207",
      title: "Saved filters per project",
      priority: "low",
      updated: "1d ago",
    },
  },
  {
    col: 1,
    slot: 0,
    issue: {
      identifier: "LIF-198",
      title: "Fix WAL checkpoint race",
      priority: "high",
      labels: [L.core, L.bug],
      updated: "26m ago",
    },
  },
  {
    col: 1,
    slot: 1,
    issue: {
      identifier: "LIF-226",
      title: "MCP: recurring plan templates",
      priority: "medium",
      labels: [L.mcp],
      updated: "2h ago",
    },
  },
  {
    col: 2,
    slot: 0,
    issue: {
      identifier: "LIF-183",
      title: "OAuth device flow for CLI",
      labels: [L.auth],
      updated: "5h ago",
      status: "done",
    },
  },
  {
    col: 2,
    slot: 1,
    issue: {
      identifier: "LIF-171",
      title: "Backup retention config",
      labels: [L.core],
      updated: "1d ago",
      status: "done",
    },
  },
];

const APP_W = 1500;
const APP_H = 860;

export const Hero: React.FC = () => {
  return (
    <AbsoluteFill style={{ backgroundColor: C.stone950 }}>
      {/* The real board UI, slightly scaled, anchored top-left */}
      <div
        style={{
          position: "absolute",
          left: -30,
          top: 30,
          transform: "scale(1.32)",
          transformOrigin: "top left",
          opacity: 0.52,
        }}
      >
        <LificApp
          width={APP_W}
          height={APP_H}
          counts={{ backlog: 2, todo: 3, active: 2, done: 2 }}
          totalLabel={"7"}
        >
          {CARDS.map(({ issue, col, slot }) => (
            <div
              key={issue.identifier}
              style={{
                position: "absolute",
                left: colX(col) + CARD_PAD,
                top: cardsTop() + slot * 95,
              }}
            >
              <IssueCard issue={issue} width={CARD_W} />
            </div>
          ))}
        </LificApp>
      </div>

      {/* Fades: darken toward the right (logo zone) and the edges */}
      <AbsoluteFill
        style={{
          background: `linear-gradient(100deg, transparent 30%, ${C.stone950}f2 72%)`,
        }}
      />
      <AbsoluteFill
        style={{
          background: `linear-gradient(to bottom, ${C.stone950}cc 0%, transparent 22%, transparent 72%, ${C.stone950}e6 100%)`,
        }}
      />

      {/* Logo + wordmark, right of center like the original */}
      <div
        style={{
          position: "absolute",
          left: 1090,
          top: 150,
          width: 640,
          display: "flex",
          flexDirection: "column",
          alignItems: "center",
          gap: 54,
        }}
      >
        <Img
          src={staticFile("logo.webp")}
          style={{
            width: 400,
            height: 400,
            borderRadius: 90,
            boxShadow: `0 0 160px ${C.accentSubtle}, 0 40px 90px rgba(0,0,0,0.6)`,
          }}
        />
        <div
          style={{
            fontFamily: DISPLAY,
            fontSize: 230,
            fontWeight: 700,
            letterSpacing: "-0.03em",
            lineHeight: 1,
            color: "#f5f7f6",
            textShadow: "0 10px 60px rgba(0,0,0,0.8)",
          }}
        >
          Lific
        </div>
      </div>
    </AbsoluteFill>
  );
};

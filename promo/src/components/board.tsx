import React from "react";
import { staticFile, Img } from "remotion";
import { C } from "../theme";
import { BODY, DISPLAY, MONO } from "../fonts";

export type CardData = {
  id: string;
  title: string;
  priority?: "high" | "medium" | "low";
  module?: string;
};

const PRIORITY_COLOR: Record<string, string> = {
  high: C.warn,
  medium: C.accent,
  low: C.textFaint,
};

export const IssueCard: React.FC<{
  card: CardData;
  width?: number;
  highlight?: number; // 0..1 green success flash
  style?: React.CSSProperties;
}> = ({ card, width = 300, highlight = 0, style }) => {
  return (
    <div
      style={{
        width,
        borderRadius: 10,
        border: `1px solid ${highlight > 0 ? C.success : C.border}`,
        backgroundColor: C.surface,
        padding: "12px 14px",
        display: "flex",
        flexDirection: "column",
        gap: 7,
        boxShadow:
          highlight > 0
            ? `0 0 ${22 * highlight}px ${C.success}55`
            : "0 2px 8px rgba(0,0,0,0.3)",
        ...style,
      }}
    >
      <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
        <span
          style={{
            fontFamily: MONO,
            fontSize: 13,
            fontWeight: 500,
            color: C.accent,
          }}
        >
          {card.id}
        </span>
        {card.priority ? (
          <span
            style={{
              width: 8,
              height: 8,
              borderRadius: 4,
              backgroundColor: PRIORITY_COLOR[card.priority],
            }}
          />
        ) : null}
        {card.module ? (
          <span
            style={{
              marginLeft: "auto",
              fontFamily: BODY,
              fontSize: 11,
              color: C.textFaint,
              border: `1px solid ${C.border}`,
              borderRadius: 999,
              padding: "1px 8px",
            }}
          >
            {card.module}
          </span>
        ) : null}
      </div>
      <div
        style={{
          fontFamily: BODY,
          fontSize: 15,
          fontWeight: 500,
          color: C.text,
          lineHeight: 1.35,
        }}
      >
        {card.title}
      </div>
    </div>
  );
};

export const Column: React.FC<{
  name: string;
  dot: string;
  count: number;
  width?: number;
  children: React.ReactNode;
}> = ({ name, dot, count, width = 330, children }) => {
  return (
    <div
      style={{
        width,
        display: "flex",
        flexDirection: "column",
        gap: 12,
      }}
    >
      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: 9,
          padding: "0 4px",
        }}
      >
        <span style={{ width: 10, height: 10, borderRadius: 5, backgroundColor: dot }} />
        <span
          style={{
            fontFamily: DISPLAY,
            fontSize: 16,
            fontWeight: 600,
            color: C.text,
          }}
        >
          {name}
        </span>
        <span style={{ fontFamily: MONO, fontSize: 13, color: C.textFaint }}>
          {count}
        </span>
      </div>
      <div style={{ display: "flex", flexDirection: "column", gap: 10 }}>
        {children}
      </div>
    </div>
  );
};

/** Sidebar + topbar shell that echoes the real app chrome. */
export const AppShell: React.FC<{
  title: string;
  active?: string;
  children: React.ReactNode;
}> = ({ title, active = "Board", children }) => {
  const items = ["Issues", "Board", "Pages", "Modules"];
  return (
    <div
      style={{
        position: "absolute",
        inset: 0,
        display: "flex",
        backgroundColor: C.bg,
      }}
    >
      <div
        style={{
          width: 230,
          backgroundColor: C.chrome,
          borderRight: `1px solid ${C.border}`,
          padding: "18px 14px",
          display: "flex",
          flexDirection: "column",
          gap: 6,
        }}
      >
        <div
          style={{
            display: "flex",
            alignItems: "center",
            gap: 10,
            padding: "4px 8px 16px",
          }}
        >
          <Img
            src={staticFile("logo.webp")}
            style={{ width: 28, height: 28, borderRadius: 7 }}
          />
          <span
            style={{
              fontFamily: DISPLAY,
              fontWeight: 600,
              fontSize: 18,
              color: C.text,
            }}
          >
            Lific
          </span>
          <span
            style={{
              marginLeft: "auto",
              fontFamily: MONO,
              fontSize: 12,
              color: C.accent,
              backgroundColor: C.accentSubtle,
              borderRadius: 6,
              padding: "2px 8px",
            }}
          >
            LIF
          </span>
        </div>
        {items.map((item) => (
          <div
            key={item}
            style={{
              fontFamily: BODY,
              fontSize: 15,
              padding: "8px 12px",
              borderRadius: 8,
              color: item === active ? C.text : C.textMuted,
              backgroundColor: item === active ? C.surface : "transparent",
              border:
                item === active
                  ? `1px solid ${C.border}`
                  : "1px solid transparent",
            }}
          >
            {item}
          </div>
        ))}
      </div>
      <div style={{ flex: 1, display: "flex", flexDirection: "column" }}>
        <div
          style={{
            height: 60,
            display: "flex",
            alignItems: "center",
            padding: "0 26px",
            borderBottom: `1px solid ${C.border}`,
            backgroundColor: C.chrome,
            fontFamily: DISPLAY,
            fontSize: 19,
            fontWeight: 600,
            color: C.text,
          }}
        >
          {title}
        </div>
        <div style={{ flex: 1, position: "relative", backgroundColor: C.bg }}>
          {children}
        </div>
      </div>
    </div>
  );
};

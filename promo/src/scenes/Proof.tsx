import React from "react";
import { AbsoluteFill } from "remotion";
import { C } from "../theme";
import { BODY, MONO } from "../fonts";
import { Background } from "../components/Background";
import { KineticLine, FadeUp } from "../components/text";

const Chip: React.FC<{ children: React.ReactNode }> = ({ children }) => (
  <span
    style={{
      fontFamily: MONO,
      fontSize: 26,
      fontWeight: 500,
      color: C.text,
      border: `1px solid ${C.border}`,
      backgroundColor: C.bgSubtle,
      borderRadius: 12,
      padding: "14px 30px",
    }}
  >
    {children}
  </span>
);

export const Proof: React.FC = () => (
  <Background>
    <AbsoluteFill
      style={{ justifyContent: "center", alignItems: "center", gap: 50 }}
    >
      <KineticLine text="Local-first. Yours." size={100} />
      <FadeUp delay={20}>
        <div
          style={{
            fontFamily: BODY,
            fontSize: 38,
            fontWeight: 500,
            color: C.textMuted,
            textAlign: "center",
            lineHeight: 1.5,
          }}
        >
          A single Rust binary.{" "}
          <span style={{ color: C.text }}>
            Your data is a SQLite file you own
          </span>{" "}
          — back it up, grep it, take it anywhere.
        </div>
      </FadeUp>
      <FadeUp delay={40}>
        <div style={{ display: "flex", gap: 20 }}>
          <Chip>CLI</Chip>
          <Chip>REST API</Chip>
          <Chip>MCP</Chip>
          <Chip>Web UI</Chip>
        </div>
      </FadeUp>
    </AbsoluteFill>
  </Background>
);

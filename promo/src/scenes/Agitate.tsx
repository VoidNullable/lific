import React from "react";
import { AbsoluteFill } from "remotion";
import { C } from "../theme";
import { BODY, DISPLAY } from "../fonts";
import { Background } from "../components/Background";
import { KineticLine, FadeUp } from "../components/text";

const FactChip: React.FC<{ children: React.ReactNode }> = ({ children }) => (
  <span
    style={{
      fontFamily: BODY,
      fontSize: 27,
      fontWeight: 500,
      color: C.textMuted,
      border: `1px solid ${C.border}`,
      backgroundColor: C.bgSubtle,
      borderRadius: 999,
      padding: "12px 28px",
    }}
  >
    {children}
  </span>
);

/** Beat 2a — Jira. Punching up is a tradition here. */
export const AgitateJira: React.FC = () => (
  <Background glow={false}>
    <AbsoluteFill
      style={{ justifyContent: "center", alignItems: "center", gap: 54 }}
    >
      <KineticLine text="Why pay for Jira?" size={104} />
      <FadeUp delay={22}>
        <div style={{ display: "flex", gap: 20 }}>
          <FactChip>$7.91 per user / month</FactChip>
          <FactChip>famously slow</FactChip>
          <FactChip>built for the enterprise, not for you</FactChip>
        </div>
      </FadeUp>
    </AbsoluteFill>
  </Background>
);

/** Beat 2b — Linear. Never attack its quality; only lock-in + price. */
export const AgitateLinear: React.FC = () => (
  <Background glow={false}>
    <AbsoluteFill
      style={{ justifyContent: "center", alignItems: "center", gap: 54 }}
    >
      <KineticLine text="Why pay for Linear?" size={104} />
      <FadeUp delay={22}>
        <div
          style={{
            display: "flex",
            flexDirection: "column",
            alignItems: "center",
            gap: 20,
          }}
        >
          <div style={{ display: "flex", gap: 20 }}>
            <FactChip>beautiful — credit where due</FactChip>
            <FactChip>$10–16 per user / month</FactChip>
          </div>
          <div style={{ display: "flex", gap: 20 }}>
            <FactChip>SaaS-only. Your issues live on their servers.</FactChip>
          </div>
        </div>
      </FadeUp>
    </AbsoluteFill>
  </Background>
);

type FossTool = { name: string; facts: string[] };

// Every number verified against each project's own docs / compose files
// on 2026-07-02. See LIF-DOC-17. Grouped on purpose: no single project
// gets singled out.
const TOOLS: FossTool[] = [
  { name: "Plane", facts: ["13 services", "8 GB RAM recommended"] },
  { name: "Taiga", facts: ["9 containers", "2 RabbitMQ instances"] },
  { name: "Huly", facts: ["14 services", "16 GB RAM recommended"] },
  { name: "OpenProject", facts: ["quad-core + 4 GB minimum", "PostgreSQL 16+"] },
];

/** Beat 2c — the FOSS field, grouped, facts from their own docs. */
export const AgitateFoss: React.FC = () => (
  <Background glow={false}>
    <AbsoluteFill
      style={{ justifyContent: "center", alignItems: "center", gap: 56 }}
    >
      <KineticLine text="Free options get heavy." size={96} />
      <FadeUp delay={20}>
        <div style={{ display: "flex", gap: 24 }}>
          {TOOLS.map((tool, i) => (
            <FadeUp key={tool.name} delay={26 + i * 7}>
              <div
                style={{
                  width: 330,
                  borderRadius: 14,
                  border: `1px solid ${C.border}`,
                  backgroundColor: C.bgSubtle,
                  padding: "26px 28px",
                  display: "flex",
                  flexDirection: "column",
                  gap: 14,
                }}
              >
                <div
                  style={{
                    fontFamily: DISPLAY,
                    fontSize: 32,
                    fontWeight: 600,
                    color: C.text,
                  }}
                >
                  {tool.name}
                </div>
                {tool.facts.map((fact) => (
                  <div
                    key={fact}
                    style={{
                      fontFamily: BODY,
                      fontSize: 21,
                      color: C.textMuted,
                    }}
                  >
                    {fact}
                  </div>
                ))}
              </div>
            </FadeUp>
          ))}
        </div>
      </FadeUp>
      <FadeUp delay={55}>
        <div style={{ fontFamily: BODY, fontSize: 19, color: C.textFaint }}>
          figures from each project&apos;s own documentation
        </div>
      </FadeUp>
    </AbsoluteFill>
  </Background>
);

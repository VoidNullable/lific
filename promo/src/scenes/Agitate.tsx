import React from "react";
import {
  AbsoluteFill,
  useCurrentFrame,
  useVideoConfig,
  spring,
  interpolate,
  staticFile,
  Img,
} from "remotion";
import { C } from "../theme";
import { BODY, DISPLAY, MONO } from "../fonts";
import { Background } from "../components/Background";
import { KineticLine, FadeUp } from "../components/text";

/*
 * Competitor logos: official marks via Simple Icons, used unmodified and
 * purely referentially (nominative fair use in truthful comparative
 * advertising). Brand colors from each company's own palette.
 */
const JIRA_PATH =
  "M11.571 11.513H0a5.218 5.218 0 0 0 5.232 5.215h2.13v2.057A5.215 5.215 0 0 0 12.575 24V12.518a1.005 1.005 0 0 0-1.005-1.005zm5.723-5.756H5.736a5.215 5.215 0 0 0 5.215 5.214h2.129v2.058a5.218 5.218 0 0 0 5.215 5.214V6.758a1.001 1.001 0 0 0-1.001-1.001zM23.013 0H11.455a5.215 5.215 0 0 0 5.215 5.215h2.129v2.057A5.215 5.215 0 0 0 24 12.483V1.005A1.001 1.001 0 0 0 23.013 0Z";
const LINEAR_PATH =
  "M2.886 4.18A11.982 11.982 0 0 1 11.99 0C18.624 0 24 5.376 24 12.009c0 3.64-1.62 6.903-4.18 9.105L2.887 4.18ZM1.817 5.626l16.556 16.556c-.524.33-1.075.62-1.65.866L.951 7.277c.247-.575.537-1.126.866-1.65ZM.322 9.163l14.515 14.515c-.71.172-1.443.282-2.195.322L0 11.358a12 12 0 0 1 .322-2.195Zm-.17 4.862 9.823 9.824a12.02 12.02 0 0 1-9.824-9.824Z";

/** Brand tile: the mark in its brand color, springing in beside the headline. */
const BrandMark: React.FC<{ path: string; color: string; delay?: number }> = ({
  path,
  color,
  delay = 4,
}) => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();
  const s = spring({
    frame: frame - delay,
    fps,
    config: { damping: 14, stiffness: 120, mass: 0.8 },
  });
  return (
    <div
      style={{
        width: 132,
        height: 132,
        borderRadius: 30,
        backgroundColor: C.bgSubtle,
        border: `1px solid ${C.border}`,
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        transform: `scale(${s})`,
        boxShadow: `0 0 70px ${color}33`,
      }}
    >
      <svg width="76" height="76" viewBox="0 0 24 24">
        <path d={path} fill={color} />
      </svg>
    </div>
  );
};

const FactChip: React.FC<{ children: React.ReactNode }> = ({ children }) => (
  <span
    style={{
      fontFamily: BODY,
      fontSize: 31,
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
      <div style={{ display: "flex", alignItems: "center", gap: 44 }}>
        <BrandMark path={JIRA_PATH} color="#0052CC" />
        <KineticLine text="Why pay for Jira?" size={104} />
      </div>
      <FadeUp delay={15} duration={14}>
        <div style={{ display: "flex", gap: 20 }}>
          <FactChip>$7.91 per user / month</FactChip>
          <FactChip>&ldquo;incredibly slow, almost unusable&rdquo;</FactChip>
          <FactChip>built for the enterprise, not for you</FactChip>
        </div>
      </FadeUp>
      <FadeUp delay={30} duration={14}>
        <div style={{ fontFamily: BODY, fontSize: 22, color: C.textFaint }}>
          quote: Hacker News, Dec 2024. A recurring complaint since 2020.
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
      <div style={{ display: "flex", alignItems: "center", gap: 44 }}>
        <BrandMark path={LINEAR_PATH} color="#5E6AD2" />
        <KineticLine text="Why pay for Linear?" size={104} />
      </div>
      <FadeUp delay={15} duration={14}>
        <div
          style={{
            display: "flex",
            flexDirection: "column",
            alignItems: "center",
            gap: 20,
          }}
        >
          <div style={{ display: "flex", gap: 20 }}>
            <FactChip>beautiful, credit where due</FactChip>
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

/*
 * Beat 2c — the FOSS field. The point is FELT, not stated: each tool's
 * real service stack rains down as container blocks that pile onto its
 * card, the card sags under the weight, and the frame thuds on every
 * wave. Stacks + figures verified against each project's own docs /
 * compose files (LIF-DOC-17). Grouped on purpose: nobody gets singled
 * out. Marks used referentially (Simple Icons paths + official PNGs).
 */
const PLANE_PATH =
  "M0 5.358a.854.854 0 0 1 1.235-.767L6.134 7.05v5.768c0 .81.456 1.553 1.179 1.915l4.42 2.218v1.692a.853.853 0 0 1-1.235.766L1.18 14.732A2.14 2.14 0 0 1 0 12.817zm6.134 0a.853.853 0 0 1 1.235-.766l4.898 2.458v5.768c0 .81.457 1.552 1.18 1.915l4.42 2.218v1.692a.853.853 0 0 1-1.235.765l-4.899-2.457v-5.769a2.14 2.14 0 0 0-1.179-1.914L6.134 7.05zm6.133 0a.853.853 0 0 1 1.235-.766l9.319 4.676A2.14 2.14 0 0 1 24 11.182v7.46a.853.853 0 0 1-1.235.766l-4.899-2.457v-5.769a2.14 2.14 0 0 0-1.179-1.914l-4.42-2.218z";
const OPENPROJECT_PATH =
  "M19.35.37h-1.86a4.628 4.628 0 0 0-4.652 4.624v5.609H4.652A4.628 4.628 0 0 0 0 15.23v3.721c0 2.569 2.083 4.679 4.652 4.679h1.86c2.57 0 4.652-2.11 4.652-4.679v-3.72c0-.063 0-.158-.005-.158H8.373v3.88c0 1.026-.835 1.886-1.861 1.886h-1.86c-1.027 0-1.861-.864-1.861-1.886V15.23a1.839 1.839 0 0 1 1.86-1.833h14.697c2.57 0 4.652-2.11 4.652-4.679V4.997A4.628 4.628 0 0 0 19.35.37Zm1.861 8.345c0 1.026-.835 1.886-1.861 1.886h-3.721V4.997a1.839 1.839 0 0 1 1.86-1.833h1.86a1.839 1.839 0 0 1 1.862 1.833zm-8.373 9.706a.236.236 0 0 0 0 .03c0 .746.629 1.344 1.396 1.344.767 0 1.395-.594 1.395-1.34a.188.188 0 0 0 0-.034v-3.35h-2.791z";

type FossTool = {
  name: string;
  icon: React.ReactNode;
  /** Real components of the stack, heaviest-sounding last. */
  blocks: string[];
  /** Verified closing fact shown once the pile settles. */
  fact: string;
};

const iconSvg = (path: string, color: string) => (
  <svg width="40" height="40" viewBox="0 0 24 24">
    <path d={path} fill={color} />
  </svg>
);

// Service names verified against each project's official compose files:
// makeplane/plane docker-compose (13: web admin space live api worker
// beat-worker migrator proxy postgres valkey rabbitmq minio),
// taigaio/taiga-docker (9: db back async async-rabbitmq front events
// events-rabbitmq protected gateway), hcengineering/huly-selfhost
// ARCHITECTURE_OVERVIEW (14 services), opf/openproject-docker-compose
// stable/17 (9: db cache proxy web worker cron seeder autoheal
// hocuspocus).
const TOOLS: FossTool[] = [
  {
    name: "Plane",
    icon: iconSvg(PLANE_PATH, "#3F76FF"),
    blocks: ["postgres", "valkey", "rabbitmq", "minio", "api", "+8 more"],
    fact: "13 services, 8 GB RAM recommended",
  },
  {
    name: "Taiga",
    icon: (
      <Img src={staticFile("taiga.png")} style={{ width: 40, height: 40 }} />
    ),
    blocks: ["postgres", "taiga-back", "async-rabbitmq", "events-rabbitmq", "+5 more"],
    fact: "9 containers, 2 RabbitMQ instances",
  },
  {
    name: "Huly",
    icon: (
      <Img src={staticFile("hulylogo.png")} style={{ width: 40, height: 40 }} />
    ),
    blocks: ["cockroachdb", "elasticsearch", "minio", "redpanda", "+10 more"],
    fact: "14 services, 16 GB RAM recommended",
  },
  {
    name: "OpenProject",
    icon: iconSvg(OPENPROJECT_PATH, "#0770B8"),
    blocks: ["postgres", "memcached", "worker", "cron", "hocuspocus", "+4 more"],
    fact: "9 services, quad-core + 4 GB minimum",
  },
];

// Build-up: the headline sits alone for a beat, THEN "heavy." booms in
// (the only shake in the scene), then the stacks pile with poise.
const HEAVY_AT = 34;
const WAVE_0 = 58;
const WAVE_GAP = 9;
const DROP_DUR = 8;
const BLOCK_H = 40; // 34px block + 6 gap
const landAt = (wave: number) => WAVE_0 + wave * WAVE_GAP + DROP_DUR;

const LINE_A_AT = 126;
const LINE_B_AT = 140;
const FOOT_AT = 158;

/** One container block dropping into a card's stack. */
const ContainerBlock: React.FC<{ label: string; wave: number }> = ({
  label,
  wave,
}) => {
  // Red marks only the "+N more" overflow chips, never real components.
  const last = label.startsWith("+");
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();
  const start = WAVE_0 + wave * WAVE_GAP;
  // Poised, not violent: soft landing, shorter fall.
  const s = spring({
    frame: frame - start,
    fps,
    config: { damping: 16, stiffness: 170, mass: 0.7 },
  });
  if (frame < start) return null;
  return (
    <div
      style={{
        height: 34,
        display: "flex",
        alignItems: "center",
        gap: 9,
        padding: "0 13px",
        borderRadius: 8,
        backgroundColor: C.bg,
        border: `1px solid ${last ? C.error : C.border}`,
        fontFamily: MONO,
        fontSize: 16,
        color: last ? C.error : C.textMuted,
        opacity: Math.min(1, s * 2),
        transform: `translateY(${(1 - s) * -50}px)`,
        boxSizing: "border-box",
      }}
    >
      <span
        style={{
          width: 8,
          height: 8,
          borderRadius: 2,
          backgroundColor: last ? C.error : C.textFaint,
        }}
      />
      {label}
    </div>
  );
};

export const AgitateFoss: React.FC = () => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();

  // "heavy." slams in red.
  const heavyS = spring({
    frame: frame - HEAVY_AT,
    fps,
    config: { damping: 13, stiffness: 220, mass: 0.6 },
  });

  // ONE frame shudder, on the "heavy." boom only.
  let thud = 0;
  if (frame >= HEAVY_AT && frame < HEAVY_AT + 10) {
    thud = Math.sin((frame - HEAVY_AT) * 2.2) * 10 * (1 - (frame - HEAVY_AT) / 10);
  }

  const lineIn = (at: number) =>
    interpolate(frame, [at, at + 14], [0, 1], {
      extrapolateLeft: "clamp",
      extrapolateRight: "clamp",
    });

  return (
    <Background glow={false}>
      <AbsoluteFill
        style={{
          justifyContent: "center",
          alignItems: "center",
          gap: 40,
          transform: `translateY(${thud}px)`,
        }}
      >
        {/* Headline: "Free options get" + red slammed "heavy." */}
        <div style={{ display: "flex", alignItems: "baseline", gap: 30 }}>
          <KineticLine text="Free options get" size={92} />
          {frame >= HEAVY_AT ? (
            <span
              style={{
                fontFamily: DISPLAY,
                fontSize: 100,
                fontWeight: 700,
                letterSpacing: "-0.02em",
                color: C.error,
                display: "inline-block",
                opacity: Math.min(1, heavyS * 2),
                transform: `scale(${1.9 - heavyS * 0.9})`,
              }}
            >
              heavy.
            </span>
          ) : null}
        </div>

        {/* The four stacks. Cards sag as blocks land on them. */}
        <div style={{ display: "flex", gap: 24, alignItems: "flex-start" }}>
          {TOOLS.map((tool, col) => {
            const landed = tool.blocks.filter(
              (_, w) => frame >= landAt(w),
            ).length;
            const sag = spring({
              frame: frame - landAt(Math.max(0, landed - 1)),
              fps,
              config: { damping: 11, stiffness: 170, mass: 0.8 },
            });
            const sagY = landed === 0 ? 0 : (landed - 1) * 3 + sag * 3;
            const enter = spring({
              frame: frame - 44 - col * 3,
              fps,
              config: { damping: 200, stiffness: 120 },
            });
            const factIn = lineIn(landAt(tool.blocks.length - 1) + 8);
            return (
              <div
                key={tool.name}
                style={{
                  width: 320,
                  opacity: enter,
                  transform: `translateY(${(1 - enter) * 24 + sagY}px)`,
                }}
              >
                <div
                  style={{
                    borderRadius: 14,
                    border: `1px solid ${C.border}`,
                    backgroundColor: C.bgSubtle,
                    padding: "20px 22px",
                    display: "flex",
                    flexDirection: "column",
                    gap: 10,
                    minHeight: 330,
                    boxSizing: "border-box",
                  }}
                >
                  <div
                    style={{
                      display: "flex",
                      alignItems: "center",
                      gap: 14,
                      paddingBottom: 8,
                    }}
                  >
                    {tool.icon}
                    <span
                      style={{
                        fontFamily: DISPLAY,
                        fontSize: 30,
                        fontWeight: 600,
                        color: C.text,
                      }}
                    >
                      {tool.name}
                    </span>
                  </div>
                  <div
                    style={{
                      display: "flex",
                      flexDirection: "column",
                      gap: 6,
                      minHeight: BLOCK_H * tool.blocks.length,
                    }}
                  >
                    {tool.blocks.map((label, w) => (
                      <ContainerBlock
                        key={`${label}-${w}`}
                        label={label}
                        wave={w}
                      />
                    ))}
                  </div>
                  {/* The takeaway line pops: bold + bright so it reads
                      even on a fast pass. */}
                  <div
                    style={{
                      marginTop: "auto",
                      fontFamily: BODY,
                      fontSize: 21,
                      fontWeight: 700,
                      color: C.text,
                      opacity: factIn,
                    }}
                  >
                    {tool.fact}
                  </div>
                </div>
              </div>
            );
          })}
        </div>

        {/* The clarifying copy: what they chase vs who is left unserved. */}
        <div
          style={{
            display: "flex",
            flexDirection: "column",
            alignItems: "center",
            gap: 10,
          }}
        >
          <div
            style={{
              fontFamily: BODY,
              fontSize: 36,
              fontWeight: 600,
              color: C.text,
              opacity: lineIn(LINE_A_AT),
              transform: `translateY(${(1 - lineIn(LINE_A_AT)) * 16}px)`,
            }}
          >
            Everything-apps, chasing the enterprise.
          </div>
          <div
            style={{
              fontFamily: BODY,
              fontSize: 30,
              fontWeight: 500,
              color: C.textMuted,
              opacity: lineIn(LINE_B_AT),
              transform: `translateY(${(1 - lineIn(LINE_B_AT)) * 16}px)`,
            }}
          >
            Nobody builds for a home server, a small team, and months of agent
            work.
          </div>
          <div
            style={{
              fontFamily: BODY,
              fontSize: 20,
              color: C.textFaint,
              opacity: lineIn(FOOT_AT),
            }}
          >
            stacks and figures from each project&apos;s own documentation
          </div>
        </div>
      </AbsoluteFill>
    </Background>
  );
};

import React from "react";
import {
  AbsoluteFill,
  useCurrentFrame,
  useVideoConfig,
  spring,
  interpolate,
} from "remotion";
import { C } from "../theme";
import { BODY, DISPLAY } from "../fonts";
import { Background } from "../components/Background";

/*
 * Ad B compressed problem beat: the whole Paid/Heavy argument in one
 * scene (~7s). All claims verified in LIF-DOC-17 (pricing pages,
 * official compose files).
 */

const JIRA_PATH =
  "M11.571 11.513H0a5.218 5.218 0 0 0 5.232 5.215h2.13v2.057A5.215 5.215 0 0 0 12.575 24V12.518a1.005 1.005 0 0 0-1.005-1.005zm5.723-5.756H5.736a5.215 5.215 0 0 0 5.215 5.214h2.129v2.058a5.218 5.218 0 0 0 5.215 5.214V6.758a1.001 1.001 0 0 0-1.001-1.001zM23.013 0H11.455a5.215 5.215 0 0 0 5.215 5.215h2.129v2.057A5.215 5.215 0 0 0 24 12.483V1.005A1.001 1.001 0 0 0 23.013 0Z";
const LINEAR_PATH =
  "M2.886 4.18A11.982 11.982 0 0 1 11.99 0C18.624 0 24 5.376 24 12.009c0 3.64-1.62 6.903-4.18 9.105L2.887 4.18ZM1.817 5.626l16.556 16.556c-.524.33-1.075.62-1.65.866L.951 7.277c.247-.575.537-1.126.866-1.65ZM.322 9.163l14.515 14.515c-.71.172-1.443.282-2.195.322L0 11.358a12 12 0 0 1 .322-2.195Zm-.17 4.862 9.823 9.824a12.02 12.02 0 0 1-9.824-9.824Z";

const PAID_AT = 40;
const ROW2_AT = 60;
const HEAVY_AT = 100;

const Slam: React.FC<{ at: number; color: string; children: string }> = ({
  at,
  color,
  children,
}) => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();
  const s = spring({
    frame: frame - at,
    fps,
    config: { damping: 13, stiffness: 220, mass: 0.6 },
  });
  if (frame < at) return null;
  return (
    <span
      style={{
        display: "inline-block",
        fontFamily: DISPLAY,
        fontSize: 96,
        fontWeight: 700,
        letterSpacing: "-0.02em",
        color,
        opacity: Math.min(1, s * 2),
        transform: `scale(${1.9 - s * 0.9})`,
      }}
    >
      {children}
    </span>
  );
};

const fadeIn = (frame: number, at: number) =>
  interpolate(frame, [at, at + 12], [0, 1], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
  });

export const ProblemFlash: React.FC = () => {
  const frame = useCurrentFrame();

  // One shudder per slam.
  const shake = (at: number) =>
    frame >= at && frame < at + 8
      ? Math.sin((frame - at) * 2.3) * 7 * (1 - (frame - at) / 8)
      : 0;
  const dy = shake(PAID_AT) + shake(HEAVY_AT);

  return (
    <Background glow={false}>
      <AbsoluteFill
        style={{
          justifyContent: "center",
          alignItems: "center",
          gap: 42,
          transform: `translateY(${dy}px)`,
        }}
      >
        <div
          style={{
            fontFamily: BODY,
            fontSize: 40,
            fontWeight: 500,
            color: C.textMuted,
            opacity: fadeIn(frame, 6),
          }}
        >
          The trackers you could be using:
        </div>

        {/* Row 1: Paid */}
        <div
          style={{
            display: "flex",
            alignItems: "center",
            gap: 40,
            opacity: fadeIn(frame, 20),
          }}
        >
          <svg width="56" height="56" viewBox="0 0 24 24">
            <path d={JIRA_PATH} fill="#0052CC" />
          </svg>
          <span style={{ fontFamily: BODY, fontSize: 34, color: C.text }}>
            $7.91 / user / month
          </span>
          <svg width="56" height="56" viewBox="0 0 24 24">
            <path d={LINEAR_PATH} fill="#5E6AD2" />
          </svg>
          <span style={{ fontFamily: BODY, fontSize: 34, color: C.text }}>
            $10&ndash;16 / user / month
          </span>
          <div style={{ width: 260, textAlign: "left" }}>
            <Slam at={PAID_AT} color={C.warn}>
              Paid.
            </Slam>
          </div>
        </div>

        {/* Row 2: Heavy */}
        <div
          style={{
            display: "flex",
            alignItems: "center",
            gap: 40,
            opacity: fadeIn(frame, ROW2_AT),
          }}
        >
          <span style={{ fontFamily: BODY, fontSize: 34, color: C.text }}>
            Plane &middot; Taiga &middot; Huly &middot; OpenProject
          </span>
          <span style={{ fontFamily: BODY, fontSize: 34, color: C.textMuted }}>
            9&ndash;14 services, 8&ndash;16 GB RAM
          </span>
          <div style={{ width: 320, textAlign: "left" }}>
            <Slam at={HEAVY_AT} color={C.error}>
              Heavy.
            </Slam>
          </div>
        </div>

        <div
          style={{
            fontFamily: BODY,
            fontSize: 21,
            color: C.textFaint,
            opacity: fadeIn(frame, 118),
          }}
        >
          their own pricing pages and compose files
        </div>
      </AbsoluteFill>
    </Background>
  );
};

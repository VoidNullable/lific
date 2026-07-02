import React from "react";
import {
  AbsoluteFill,
  useCurrentFrame,
  useVideoConfig,
  spring,
  interpolate,
  Easing,
} from "remotion";
import { C } from "../theme";
import { BODY } from "../fonts";
import { Background } from "../components/Background";
import { BrowserFrame } from "../components/BrowserFrame";
import { InstanceSettingsPage } from "../components/instance-settings-ui";
import { Cursor, Waypoint } from "../components/Cursor";

/*
 * Pixel-faithful replica of Lific's Instance Settings page (web/src/routes/
 * InstanceSettings.svelte), with two "big gesture" toggle flips that show off
 * Lific 2.0's team features:
 *   1. Sign-ups  : Closed -> Open   (cursor clicks the "Open" segment)
 *   2. Project permissions : Off -> Enforced (cursor clicks "Enforced")
 * Camera: ONE zoom-in on the first click that never releases — after it the
 * shot only drifts (slow push-in), panning to the second control during the
 * scroll. All motion is a pure function of useCurrentFrame().
 */

// Winning copy from the adversarial writer/judge round (see LIF-260) —
// visible for the entire scene.
const COPY_HEADLINE = "Team-ready. Still yours alone.";
const COPY_SUB = "Open signups, scope projects to members — or stay solo on the same binary.";

// ── App geometry (native CSS px), zoomed for phone legibility ─
const APP_W = 1180;
const APP_H = 640;
const SCALE = 1.4;

// Control centers in APP-LOCAL coordinates (calibrated against stills).
// SIGNUPS_OPEN is the "Open" segment (left); AUTHZ_ENFORCED is the
// "Enforced" segment (right, position AFTER the page scroll).
const SIGNUPS_OPEN = { x: 311, y: 445 };
const AUTHZ_ENFORCED = { x: 402, y: 403 };
const SCROLL_MAX = 430;

// ── Beat timing (scene-local frames, 0..269) ─────────────────
const SIGNUPS_CLICK = 44;
const SCROLL_START = 96;
const SCROLL_END = 124;
const AUTHZ_CLICK = 138;
const CURSOR_EXIT = AUTHZ_CLICK + 34;

const ease = Easing.bezier(0.4, 0, 0.2, 1);

/** Short colored bloom radiating from a control after its flip. */
const Bloom: React.FC<{
  at: number;
  x: number;
  y: number;
  color: string;
}> = ({ at, x, y, color }) => {
  const frame = useCurrentFrame();
  const life = interpolate(frame, [at, at + 10, at + 44], [0, 0.55, 0], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
    easing: ease,
  });
  const grow = interpolate(frame, [at, at + 44], [0.5, 1.7], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
    easing: ease,
  });
  if (life <= 0) return null;
  return (
    <div
      style={{
        position: "absolute",
        left: x - 170,
        top: y - 170,
        width: 340,
        height: 340,
        borderRadius: 170,
        background: `radial-gradient(circle, ${color}55 0%, ${color}22 40%, transparent 70%)`,
        opacity: life,
        transform: `scale(${grow})`,
        pointerEvents: "none",
      }}
    />
  );
};

/** Confirmation pill that springs in beside a flipped control. */
const ConfirmPill: React.FC<{
  at: number;
  x: number;
  y: number;
  color: string;
  children: React.ReactNode;
  fadeAt?: number;
}> = ({ at, x, y, color, children, fadeAt }) => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();
  const s = spring({ frame: frame - at, fps, config: { damping: 14, stiffness: 160, mass: 0.6 } });
  const fade =
    fadeAt === undefined
      ? 1
      : interpolate(frame, [fadeAt, fadeAt + 12], [1, 0], {
          extrapolateLeft: "clamp",
          extrapolateRight: "clamp",
        });
  if (frame < at) return null;
  return (
    <div
      style={{
        position: "absolute",
        left: x,
        top: y,
        display: "flex",
        alignItems: "center",
        gap: 8,
        padding: "8px 16px",
        borderRadius: 999,
        backgroundColor: C.bgSubtle,
        border: `1px solid ${color}`,
        boxShadow: `0 0 24px ${color}44, 0 4px 14px rgba(0,0,0,0.4)`,
        fontFamily: BODY,
        fontSize: 16,
        fontWeight: 600,
        color,
        opacity: s * fade,
        transform: `scale(${0.85 + s * 0.15}) translateY(${(1 - s) * 10}px)`,
        pointerEvents: "none",
        whiteSpace: "nowrap",
      }}
    >
      {children}
    </div>
  );
};

export const TeamsScene: React.FC = () => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();

  // Page springs in.
  const frameIn = spring({ frame, fps, config: { damping: 200, stiffness: 90 } });

  // ── Flip progress (0..1), springs kicked at each click ──────
  const signupsProgress = spring({
    frame: frame - SIGNUPS_CLICK,
    fps,
    config: { damping: 15, stiffness: 180, mass: 0.7 },
  });
  const authzProgress = spring({
    frame: frame - AUTHZ_CLICK,
    fps,
    config: { damping: 15, stiffness: 180, mass: 0.7 },
  });

  // ── Camera: ONE zoom-in on the first click, held forever, with a
  // slow cinematic push-in afterwards. No zoom-out, no second punch. ──
  const zoomIn = interpolate(frame, [SIGNUPS_CLICK, SIGNUPS_CLICK + 18], [0, 1], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
    easing: ease,
  });
  const drift = interpolate(frame, [SIGNUPS_CLICK + 18, 270], [0, 0.03], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
  });
  const zoom = 1 + zoomIn * 0.16 + drift;

  // Page scroll between the two gestures (while zoomed).
  const scrollY = interpolate(frame, [SCROLL_START, SCROLL_END], [0, SCROLL_MAX], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
    easing: ease,
  });

  // Focal point pans from the first control to the second during the scroll.
  const focalX = interpolate(frame, [SCROLL_START, SCROLL_END], [SIGNUPS_OPEN.x, AUTHZ_ENFORCED.x], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
    easing: ease,
  });
  const focalY = interpolate(frame, [SCROLL_START, SCROLL_END], [SIGNUPS_OPEN.y, AUTHZ_ENFORCED.y], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
    easing: ease,
  });

  const focalDx = (APP_W / 2 - focalX) * (zoom - 1);
  const focalDy = (APP_H / 2 - focalY) * (zoom - 1);

  // Brief white flash on each click (4% overlay, 6 frames).
  const flash = Math.max(
    interpolate(frame, [SIGNUPS_CLICK, SIGNUPS_CLICK + 3, SIGNUPS_CLICK + 8], [0, 0.05, 0], {
      extrapolateLeft: "clamp",
      extrapolateRight: "clamp",
    }),
    interpolate(frame, [AUTHZ_CLICK, AUTHZ_CLICK + 3, AUTHZ_CLICK + 8], [0, 0.05, 0], {
      extrapolateLeft: "clamp",
      extrapolateRight: "clamp",
    }),
  );

  // ── Cursor script: click "Open", then "Enforced" ────────────
  const CURSOR: Waypoint[] = [
    { at: 8, x: 320, y: 220 },
    { at: SIGNUPS_CLICK - 5, x: SIGNUPS_OPEN.x, y: SIGNUPS_OPEN.y },
    { at: SIGNUPS_CLICK, x: SIGNUPS_OPEN.x, y: SIGNUPS_OPEN.y, click: true },
    { at: SCROLL_END + 4, x: AUTHZ_ENFORCED.x + 60, y: AUTHZ_ENFORCED.y + 60 },
    { at: AUTHZ_CLICK - 5, x: AUTHZ_ENFORCED.x, y: AUTHZ_ENFORCED.y },
    { at: AUTHZ_CLICK, x: AUTHZ_ENFORCED.x, y: AUTHZ_ENFORCED.y, click: true },
    { at: CURSOR_EXIT, x: AUTHZ_ENFORCED.x + 150, y: AUTHZ_ENFORCED.y + 100 },
  ];

  // ── Caption: visible for the whole scene ────────────────────
  const captionIn = interpolate(frame, [6, 22], [0, 1], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
  });

  const framedScale = SCALE * zoom * (0.985 + frameIn * 0.015);

  return (
    <Background>
      <AbsoluteFill style={{ justifyContent: "center", alignItems: "center" }}>
        <div
          style={{
            transform: `translate(${focalDx * SCALE}px, ${focalDy * SCALE}px) scale(${framedScale})`,
            opacity: frameIn,
            marginTop: -46,
          }}
        >
          <BrowserFrame url="localhost:8080/#/settings/instance" width={APP_W} height={APP_H + 52}>
            <div style={{ position: "relative" }}>
              <InstanceSettingsPage
                width={APP_W}
                height={APP_H}
                host="localhost:8080"
                signupsProgress={signupsProgress}
                authzProgress={authzProgress}
                scrollY={scrollY}
              />

              {/* Flip effects: colored bloom + confirmation pill */}
              <Bloom
                at={SIGNUPS_CLICK + 2}
                x={SIGNUPS_OPEN.x}
                y={SIGNUPS_OPEN.y}
                color={C.success}
              />
              <ConfirmPill
                at={SIGNUPS_CLICK + 10}
                x={SIGNUPS_OPEN.x + 210}
                y={SIGNUPS_OPEN.y - 18}
                color={C.success}
                fadeAt={SCROLL_START}
              >
                ✓ Teammates can join
              </ConfirmPill>

              <Bloom
                at={AUTHZ_CLICK + 2}
                x={AUTHZ_ENFORCED.x}
                y={AUTHZ_ENFORCED.y}
                color={C.warn}
              />
              <ConfirmPill
                at={AUTHZ_CLICK + 10}
                x={AUTHZ_ENFORCED.x + 210}
                y={AUTHZ_ENFORCED.y - 18}
                color={C.warn}
              >
                ✓ Members-only projects
              </ConfirmPill>

              <Cursor points={CURSOR} />
            </div>
          </BrowserFrame>
        </div>

        {/* Click flash */}
        {flash > 0 ? (
          <AbsoluteFill style={{ backgroundColor: `rgba(255,255,255,${flash})` }} />
        ) : null}

        {/* Scrim so the caption never fights the app frame's bottom edge */}
        <div
          style={{
            position: "absolute",
            left: 0,
            right: 0,
            bottom: 0,
            height: 240,
            background: `linear-gradient(to bottom, transparent, ${C.bg}ee 62%)`,
            opacity: captionIn,
          }}
        />
        {/* Caption — on screen for the whole scene */}
        <div
          style={{
            position: "absolute",
            bottom: 40,
            width: "100%",
            textAlign: "center",
            fontFamily: BODY,
            opacity: captionIn,
            textShadow: "0 4px 30px rgba(0,0,0,0.9)",
          }}
        >
          <div style={{ fontSize: 44, fontWeight: 600, color: C.text, lineHeight: 1.1 }}>
            {COPY_HEADLINE}
          </div>
          <div style={{ fontSize: 30, color: C.textMuted, marginTop: 6, lineHeight: 1.2 }}>
            {COPY_SUB}
          </div>
        </div>
      </AbsoluteFill>
    </Background>
  );
};

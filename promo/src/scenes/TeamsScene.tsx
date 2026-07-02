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
 *   1. Sign-ups  : Closed -> Open   (green, permissive)
 *   2. Project permissions : Off -> Enforced (amber, membership-gated)
 * Each click triggers a spring toggle flip AND a punch-zoom easing toward the
 * control. All motion is a pure function of useCurrentFrame().
 */

// Final copy is being written separately — these get swapped.
// Winning copy from the adversarial writer/judge round (see LIF-260):
// B5 headline (only candidate carrying both halves alone) + B3-derived
// sub grounded in the single-binary identity, trimmed to 13 words.
const COPY_HEADLINE = "Team-ready. Still yours alone.";
const COPY_SUB = "Open signups, scope projects to members — or stay solo on the same binary.";

// ── App geometry (native CSS px), zoomed for phone legibility ─
const APP_W = 1180;
const APP_H = 640;
const SCALE = 1.4;

// The two controls' centers, in APP-LOCAL coordinates (inside the app frame,
// origin at the app's top-left). Content column is centered in the region to
// the right of the 230px sidebar; these were tuned against rendered stills.
// x: sidebar(230) + contentLeftPad -> card -> toggle. y: topbar(44) + panel.
// The Project-permissions control lives far down the form, so the page scrolls
// (SCROLL_MAX) before the second gesture — AUTHZ_CTRL.y is its on-screen y
// AFTER that scroll.
const SIGNUPS_CTRL = { x: 410, y: 445 };
const AUTHZ_CTRL = { x: 422, y: 415 };
const SCROLL_MAX = 430;

// ── Beat timing (scene-local frames, 0..199) ─────────────────
const SIGNUPS_CLICK = 42;
const SCROLL_START = 62;
const SCROLL_END = 86;
const AUTHZ_CLICK = 92;
const CAPTION_START = 148;

const ease = Easing.bezier(0.4, 0, 0.2, 1);

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

  // ── Punch-zoom: a quick scale-up eased toward the control on
  // each click, held briefly, settling back. Pure function of frame. ─────
  const punch = (clickAt: number) => {
    // 0 before click, ramps to 1 (peak), holds, returns to 0.
    return interpolate(
      frame,
      [clickAt - 2, clickAt + 6, clickAt + 16, clickAt + 30],
      [0, 1, 1, 0],
      { extrapolateLeft: "clamp", extrapolateRight: "clamp", easing: ease },
    );
  };
  const p1 = punch(SIGNUPS_CLICK);
  const p2 = punch(AUTHZ_CLICK);

  // Page scrolls down between the two gestures to bring Project permissions
  // into view.
  const scrollY = interpolate(frame, [SCROLL_START, SCROLL_END], [0, SCROLL_MAX], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
    easing: ease,
  });

  // Combined zoom + focal target. Only one punch is active at a time.
  const punchAmt = Math.max(p1, p2);
  const focal =
    p2 > p1 ? AUTHZ_CTRL : SIGNUPS_CTRL;

  const PUNCH_MAX = 0.14; // +14%
  const zoom = 1 + punchAmt * PUNCH_MAX;

  // Translate the focal point toward center as we zoom, so the control stays
  // framed. Focal offset from app center, scaled by the extra zoom.
  const appCx = APP_W / 2;
  const appCy = APP_H / 2;
  const focalDx = (appCx - focal.x) * (zoom - 1);
  const focalDy = (appCy - focal.y) * (zoom - 1);

  // ── Cursor script ───────────────────────────────────────────
  const CURSOR: Waypoint[] = [
    { at: 6, x: 300, y: 210 },
    { at: SIGNUPS_CLICK - 4, x: SIGNUPS_CTRL.x, y: SIGNUPS_CTRL.y },
    { at: SIGNUPS_CLICK, x: SIGNUPS_CTRL.x, y: SIGNUPS_CTRL.y, click: true },
    { at: AUTHZ_CLICK - 6, x: AUTHZ_CTRL.x, y: AUTHZ_CTRL.y },
    { at: AUTHZ_CLICK, x: AUTHZ_CTRL.x, y: AUTHZ_CTRL.y, click: true },
    { at: AUTHZ_CLICK + 34, x: AUTHZ_CTRL.x + 140, y: AUTHZ_CTRL.y + 80 },
  ];

  // ── Caption fade-in ─────────────────────────────────────────
  const captionIn = interpolate(frame, [CAPTION_START, CAPTION_START + 16], [0, 1], {
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
            marginTop: -30,
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
              <Cursor points={CURSOR} />
            </div>
          </BrowserFrame>
        </div>

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
        {/* Bottom caption — winning copy from the adversarial round */}
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

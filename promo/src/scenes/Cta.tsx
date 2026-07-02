import React from "react";
import {
  AbsoluteFill,
  useCurrentFrame,
  useVideoConfig,
  spring,
  staticFile,
  Img,
} from "remotion";
import { C, CTA_URL } from "../theme";
import { BODY, DISPLAY, MONO } from "../fonts";
import { Background } from "../components/Background";
import { FadeUp } from "../components/text";

export const Cta: React.FC = () => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();

  const logoIn = spring({ frame, fps, config: { damping: 200, stiffness: 100 } });
  const urlIn = spring({
    frame: frame - 10,
    fps,
    config: { damping: 16, stiffness: 110 },
  });
  // Lizzy peeks in from the bottom-right corner, then gently bobs
  // through the long hold.
  const lizzy = spring({
    frame: frame - 28,
    fps,
    config: { damping: 15, stiffness: 90 },
  });
  const bob = Math.sin((frame - 28) / 34) * 6;

  // Slow breathing glow on the CTA box so the extended hold stays alive.
  const breathe = (Math.sin(frame / 26) + 1) / 2; // 0..1

  return (
    <Background>
      <AbsoluteFill
        style={{ justifyContent: "center", alignItems: "center", gap: 44 }}
      >
        <div
          style={{
            display: "flex",
            alignItems: "center",
            gap: 22,
            opacity: logoIn,
            transform: `translateY(${(1 - logoIn) * 24}px)`,
          }}
        >
          <Img
            src={staticFile("logo.webp")}
            style={{ width: 72, height: 72, borderRadius: 16 }}
          />
          <span
            style={{
              fontFamily: DISPLAY,
              fontSize: 64,
              fontWeight: 700,
              letterSpacing: "-0.02em",
              color: C.text,
            }}
          >
            Lific
          </span>
        </div>

        {/* The one CTA. Swap CTA_URL in theme.ts when lific.dev is live. */}
        <div
          style={{
            fontFamily: MONO,
            fontSize: 58,
            fontWeight: 600,
            color: C.text,
            backgroundColor: C.accentSubtle,
            border: `1px solid ${C.accentDeep}`,
            borderRadius: 18,
            padding: "26px 54px",
            opacity: urlIn,
            transform: `scale(${0.92 + urlIn * 0.08 + breathe * 0.008})`,
            boxShadow: `0 0 ${60 + breathe * 40}px ${C.accentSubtle}`,
          }}
        >
          {CTA_URL}
        </div>

        <FadeUp delay={24} duration={14}>
          <div style={{ fontFamily: BODY, fontSize: 30, color: C.textMuted }}>
            Free &amp; open source. Your issues, your machine.
          </div>
        </FadeUp>
      </AbsoluteFill>

      <Img
        src={staticFile("LizzyWriting.png")}
        style={{
          position: "absolute",
          right: 70,
          bottom: 28 + bob + (1 - lizzy) * -220,
          width: 260,
          opacity: lizzy,
        }}
      />
    </Background>
  );
};

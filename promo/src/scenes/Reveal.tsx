import React from "react";
import {
  AbsoluteFill,
  useCurrentFrame,
  useVideoConfig,
  spring,
  staticFile,
  Img,
} from "remotion";
import { C } from "../theme";
import { BODY, DISPLAY } from "../fonts";
import { Background } from "../components/Background";
import { FadeUp } from "../components/text";

export const Reveal: React.FC = () => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();

  const logoIn = spring({
    frame,
    fps,
    config: { damping: 15, stiffness: 90, mass: 0.8 },
  });
  const nameIn = spring({
    frame: frame - 8,
    fps,
    config: { damping: 200, stiffness: 100 },
  });

  return (
    <Background>
      <AbsoluteFill
        style={{ justifyContent: "center", alignItems: "center", gap: 40 }}
      >
        <div style={{ display: "flex", alignItems: "center", gap: 36 }}>
          <Img
            src={staticFile("logo.webp")}
            style={{
              width: 150,
              height: 150,
              borderRadius: 32,
              transform: `scale(${logoIn})`,
              boxShadow: `0 0 90px ${C.accentSubtle}`,
            }}
          />
          <div
            style={{
              fontFamily: DISPLAY,
              fontSize: 170,
              fontWeight: 700,
              letterSpacing: "-0.03em",
              color: C.text,
              opacity: nameIn,
              transform: `translateY(${(1 - nameIn) * 40}px)`,
            }}
          >
            Lific
          </div>
        </div>
        <FadeUp delay={26}>
          <div
            style={{
              fontFamily: BODY,
              fontSize: 42,
              fontWeight: 500,
              color: C.textMuted,
              display: "flex",
              gap: 26,
              alignItems: "center",
            }}
          >
            <span style={{ color: C.text }}>One binary.</span>
            <Dot />
            <span style={{ color: C.text }}>One SQLite file.</span>
            <Dot />
            <span style={{ color: C.success }}>Free &amp; open source.</span>
          </div>
        </FadeUp>
      </AbsoluteFill>
    </Background>
  );
};

const Dot: React.FC = () => (
  <span
    style={{
      width: 9,
      height: 9,
      borderRadius: 5,
      backgroundColor: C.textFaint,
      display: "inline-block",
    }}
  />
);

import React from "react";
import { AbsoluteFill, staticFile, Img } from "remotion";
import { C } from "./theme";
import { DISPLAY } from "./fonts";

/*
 * README hero image and the site's og.png: a screenshot of the 3.0 board,
 * dimmed behind the logo and the Space Grotesk wordmark. The screenshot is
 * the running app on a seeded demo project (1600x1000 at 2x), not a
 * replica, so it cannot drift from the interface. Rendered as a Still:
 *   bunx remotion still Hero ../LificHero.png
 * then copy the result to site/public/og.png. Everything is static, so
 * frame 0 is the image.
 */

// The screenshot's CSS width, and how much the hero enlarges it.
const SHOT_W = 1600;
const SHOT_SCALE = 1.32;

export const Hero: React.FC = () => {
  return (
    <AbsoluteFill style={{ backgroundColor: C.stone950 }}>
      {/* The board, slightly enlarged, anchored top-left */}
      <Img
        src={staticFile("board-3.0.png")}
        style={{
          position: "absolute",
          left: -30,
          top: 30,
          width: SHOT_W * SHOT_SCALE,
          opacity: 0.52,
        }}
      />

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

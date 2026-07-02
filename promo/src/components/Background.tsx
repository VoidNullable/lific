import React from "react";
import { AbsoluteFill, useCurrentFrame } from "remotion";
import { noise2D } from "@remotion/noise";
import { C } from "../theme";

/**
 * Brand background: sage near-black floor with a slow-drifting indigo
 * glow. All motion derives from useCurrentFrame(); noise is deterministic.
 */
export const Background: React.FC<{
  glow?: boolean;
  children?: React.ReactNode;
}> = ({ glow = true, children }) => {
  const frame = useCurrentFrame();
  const dx = noise2D("glow-x", frame / 300, 0) * 140;
  const dy = noise2D("glow-y", 0, frame / 300) * 90;

  return (
    <AbsoluteFill style={{ backgroundColor: C.bg }}>
      {glow ? (
        <AbsoluteFill
          style={{
            background: `radial-gradient(900px 600px at ${960 + dx}px ${420 + dy}px, ${C.accentSubtle}88, transparent 70%)`,
          }}
        />
      ) : null}
      <AbsoluteFill
        style={{
          background:
            "radial-gradient(1400px 900px at 50% 50%, transparent 55%, rgba(0,0,0,0.35) 100%)",
        }}
      />
      {children}
    </AbsoluteFill>
  );
};

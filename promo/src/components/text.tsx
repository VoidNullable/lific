import React from "react";
import { useCurrentFrame, useVideoConfig, spring, interpolate } from "remotion";
import { C } from "../theme";
import { DISPLAY } from "../fonts";

/** Headline with per-word staggered spring entrance. */
export const KineticLine: React.FC<{
  text: string;
  delay?: number;
  size?: number;
  color?: string;
  weight?: number;
  stagger?: number;
}> = ({ text, delay = 0, size = 88, color = C.text, weight = 700, stagger = 4 }) => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();
  const words = text.split(" ");
  return (
    <div
      style={{
        display: "flex",
        flexWrap: "wrap",
        justifyContent: "center",
        columnGap: size * 0.28,
        fontFamily: DISPLAY,
        fontSize: size,
        fontWeight: weight,
        letterSpacing: "-0.02em",
        lineHeight: 1.15,
        color,
      }}
    >
      {words.map((word, i) => {
        const s = spring({
          frame: frame - delay - i * stagger,
          fps,
          config: { damping: 200, stiffness: 120 },
        });
        return (
          <span
            key={i}
            style={{
              display: "inline-block",
              opacity: s,
              transform: `translateY(${(1 - s) * size * 0.4}px)`,
            }}
          >
            {word}
          </span>
        );
      })}
    </div>
  );
};

/** Simple fade+rise for secondary copy. */
export const FadeUp: React.FC<{
  delay?: number;
  duration?: number;
  children: React.ReactNode;
  style?: React.CSSProperties;
}> = ({ delay = 0, duration = 18, children, style }) => {
  const frame = useCurrentFrame();
  const t = interpolate(frame, [delay, delay + duration], [0, 1], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
  });
  return (
    <div
      style={{
        opacity: t,
        transform: `translateY(${(1 - t) * 22}px)`,
        ...style,
      }}
    >
      {children}
    </div>
  );
};

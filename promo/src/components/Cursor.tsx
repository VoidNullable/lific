import React from "react";
import { useCurrentFrame, interpolate, Easing } from "remotion";
import { C } from "../theme";

export type Waypoint = { at: number; x: number; y: number; click?: boolean };

const ease = Easing.bezier(0.4, 0, 0.2, 1);

/** Interpolate cursor position across waypoints (frames relative to scene). */
export const cursorPos = (
  frame: number,
  points: Waypoint[],
): { x: number; y: number } => {
  if (points.length === 0) return { x: 0, y: 0 };
  const frames = points.map((p) => p.at);
  const x = interpolate(frame, frames, points.map((p) => p.x), {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
    easing: ease,
  });
  const y = interpolate(frame, frames, points.map((p) => p.y), {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
    easing: ease,
  });
  return { x, y };
};

export const Cursor: React.FC<{ points: Waypoint[] }> = ({ points }) => {
  const frame = useCurrentFrame();
  const { x, y } = cursorPos(frame, points);

  // Pulse on click waypoints.
  let pulse = 0;
  for (const p of points) {
    if (p.click && frame >= p.at && frame < p.at + 14) {
      pulse = 1 - (frame - p.at) / 14;
    }
  }

  return (
    <div style={{ position: "absolute", left: x, top: y, zIndex: 50 }}>
      {pulse > 0 ? (
        <div
          style={{
            position: "absolute",
            left: -18,
            top: -18,
            width: 36,
            height: 36,
            borderRadius: 18,
            border: `3px solid ${C.accent}`,
            opacity: pulse * 0.8,
            transform: `scale(${1 + (1 - pulse) * 1.2})`,
          }}
        />
      ) : null}
      <svg width="30" height="30" viewBox="0 0 24 24" style={{ display: "block" }}>
        <path
          d="M5.5 3.2 L5.5 17.5 L9.2 14.2 L11.6 19.6 L14.2 18.4 L11.8 13.1 L16.8 12.8 Z"
          fill="#f5f7f6"
          stroke="#10141288"
          strokeWidth="1.4"
        />
      </svg>
    </div>
  );
};

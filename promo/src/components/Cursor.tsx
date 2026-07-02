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

/**
 * macOS-style pointer: black core, crisp white outline, soft shadow —
 * the exact silhouette of the system arrow (tip hotspot at the
 * waypoint). Click adds a press-scale + accent ripple.
 */
export const Cursor: React.FC<{ points: Waypoint[] }> = ({ points }) => {
  const frame = useCurrentFrame();
  const { x, y } = cursorPos(frame, points);

  // Ripple + press on click waypoints.
  let pulse = 0;
  let press = 0;
  for (const p of points) {
    if (p.click && frame >= p.at && frame < p.at + 16) {
      pulse = 1 - (frame - p.at) / 16;
    }
    if (p.click && frame >= p.at - 2 && frame < p.at + 6) {
      // quick down-up scale around the click moment
      press = interpolate(frame, [p.at - 2, p.at + 1, p.at + 6], [0, 1, 0], {
        extrapolateLeft: "clamp",
        extrapolateRight: "clamp",
      });
    }
  }

  return (
    <div style={{ position: "absolute", left: x, top: y, zIndex: 50 }}>
      {pulse > 0 ? (
        <div
          style={{
            position: "absolute",
            left: -16,
            top: -16,
            width: 36,
            height: 36,
            borderRadius: 18,
            border: `2.5px solid ${C.accent}`,
            opacity: pulse * 0.75,
            transform: `scale(${1 + (1 - pulse) * 1.3})`,
          }}
        />
      ) : null}
      <svg
        width="23"
        height="34"
        viewBox="0 0 14 21"
        style={{
          display: "block",
          marginLeft: -1,
          marginTop: -1,
          transform: `scale(${1 - press * 0.12})`,
          transformOrigin: "3px 3px",
          filter:
            "drop-shadow(0 1px 1.5px rgba(0,0,0,0.5)) drop-shadow(0 3px 6px rgba(0,0,0,0.3))",
        }}
      >
        {/* macOS arrow silhouette: straight left edge, notch, tail prong */}
        <path
          d="M 0.7 0.7 L 0.7 17.0 L 4.3 13.5 L 6.8 19.6 L 9.5 18.5 L 7.0 12.4 L 12.4 12.4 Z"
          fill="#000000"
          stroke="#ffffff"
          strokeWidth="1.3"
          strokeLinejoin="round"
        />
      </svg>
    </div>
  );
};

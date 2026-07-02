import React from "react";
import { AbsoluteFill, useCurrentFrame, interpolate } from "remotion";
import { GENERIC } from "../theme";
import { BODY, DISPLAY } from "../fonts";

/**
 * Cold open: a deliberately generic corporate tracker stuck on a spinner.
 * No logo, no Lific branding — the pain, muted-legible in frame one.
 */
export const ColdOpen: React.FC = () => {
  const frame = useCurrentFrame();
  const spin = (frame * 9) % 360;
  const askOpacity = interpolate(frame, [45, 60], [0, 1], {
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
  });
  // Progress bar that creeps and stalls at 34%.
  const progress = interpolate(frame, [0, 30, 100], [8, 31, 34], {
    extrapolateRight: "clamp",
  });
  const shimmer = (frame * 16) % 900;

  return (
    <AbsoluteFill
      style={{
        backgroundColor: GENERIC.bg,
        justifyContent: "center",
        alignItems: "center",
      }}
    >
      {/* Skeleton UI rows */}
      <div
        style={{
          width: 900,
          borderRadius: 14,
          border: `1px solid ${GENERIC.border}`,
          backgroundColor: GENERIC.surface,
          padding: 34,
          display: "flex",
          flexDirection: "column",
          gap: 18,
        }}
      >
        <div
          style={{
            display: "flex",
            alignItems: "center",
            gap: 14,
            paddingBottom: 12,
            borderBottom: `1px solid ${GENERIC.border}`,
          }}
        >
          <div
            style={{
              width: 34,
              height: 34,
              borderRadius: 17,
              border: `4px solid ${GENERIC.border}`,
              borderTopColor: GENERIC.blue,
              transform: `rotate(${spin}deg)`,
            }}
          />
          <span
            style={{
              fontFamily: BODY,
              fontSize: 21,
              color: GENERIC.muted,
            }}
          >
            Loading your workspace…
          </span>
          <span
            style={{
              marginLeft: "auto",
              fontFamily: BODY,
              fontSize: 16,
              color: GENERIC.muted,
            }}
          >
            {Math.round(progress)}%
          </span>
        </div>
        <div
          style={{
            height: 8,
            borderRadius: 4,
            backgroundColor: GENERIC.border,
            overflow: "hidden",
          }}
        >
          <div
            style={{
              width: `${progress}%`,
              height: "100%",
              backgroundColor: GENERIC.blue,
            }}
          />
        </div>
        {[520, 760, 430, 660, 580].map((w, i) => (
          <div
            key={i}
            style={{
              width: w,
              height: 20,
              borderRadius: 6,
              backgroundColor: GENERIC.border,
              position: "relative",
              overflow: "hidden",
            }}
          >
            <div
              style={{
                position: "absolute",
                left: shimmer - 300 - i * 60,
                top: 0,
                width: 200,
                height: "100%",
                background:
                  "linear-gradient(90deg, transparent, rgba(255,255,255,0.07), transparent)",
              }}
            />
          </div>
        ))}
      </div>

      <div
        style={{
          position: "absolute",
          bottom: 150,
          fontFamily: DISPLAY,
          fontSize: 76,
          fontWeight: 700,
          letterSpacing: "-0.02em",
          color: "#f5f7f6",
          opacity: askOpacity,
          textShadow: "0 6px 40px rgba(0,0,0,0.8)",
        }}
      >
        Still loading?
      </div>
    </AbsoluteFill>
  );
};

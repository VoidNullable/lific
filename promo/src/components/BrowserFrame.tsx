import React from "react";
import { C } from "../theme";
import { MONO } from "../fonts";

export const BrowserFrame: React.FC<{
  url: string;
  width: number;
  height: number;
  children: React.ReactNode;
}> = ({ url, width, height, children }) => {
  return (
    <div
      style={{
        width,
        height,
        borderRadius: 16,
        border: `1px solid ${C.border}`,
        backgroundColor: C.bg,
        boxShadow: "0 30px 90px rgba(0,0,0,0.6)",
        overflow: "hidden",
        display: "flex",
        flexDirection: "column",
      }}
    >
      <div
        style={{
          height: 52,
          display: "flex",
          alignItems: "center",
          gap: 10,
          padding: "0 18px",
          backgroundColor: C.chrome,
          borderBottom: `1px solid ${C.border}`,
        }}
      >
        {["#f87171", "#fbbf24", "#4ade80"].map((c) => (
          <div
            key={c}
            style={{ width: 13, height: 13, borderRadius: 7, backgroundColor: c }}
          />
        ))}
        <div
          style={{
            marginLeft: 16,
            flex: 1,
            maxWidth: 560,
            height: 32,
            borderRadius: 8,
            backgroundColor: C.bgSubtle,
            border: `1px solid ${C.border}`,
            display: "flex",
            alignItems: "center",
            padding: "0 14px",
            fontFamily: MONO,
            fontSize: 15,
            color: C.textMuted,
          }}
        >
          {url}
        </div>
      </div>
      <div style={{ flex: 1, position: "relative", overflow: "hidden" }}>
        {children}
      </div>
    </div>
  );
};

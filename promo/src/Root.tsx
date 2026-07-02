import React from "react";
import { Composition } from "remotion";
import { Ad } from "./Ad";
import { TOTAL_FRAMES } from "./timing";
import { FPS, WIDTH, HEIGHT } from "./theme";
import "./index.css";

export const RemotionRoot: React.FC = () => {
  return (
    <Composition
      id="Ad"
      component={Ad}
      durationInFrames={TOTAL_FRAMES}
      fps={FPS}
      width={WIDTH}
      height={HEIGHT}
    />
  );
};

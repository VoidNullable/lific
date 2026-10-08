import React from "react";
import { Composition, Still } from "remotion";
import { Ad } from "./Ad";
import { AdB, TOTAL_FRAMES_B } from "./AdB";
import { Hero } from "./Hero";
import {
  BoardLoop,
  BOARD_LOOP_W,
  BOARD_LOOP_H,
  BOARD_LOOP_FRAMES,
} from "./BoardLoop";
import {
  PlanSync,
  PLAN_SYNC_W,
  PLAN_SYNC_H,
  PLAN_SYNC_FRAMES,
} from "./PlanSync";
import {
  EveryoneSplit,
  EVERYONE_SPLIT_W,
  EVERYONE_SPLIT_H,
  EVERYONE_SPLIT_FRAMES,
} from "./EveryoneSplit";
import {
  AgentsStill,
  AGENTS_STILL_W,
  AGENTS_STILL_H,
  AGENTS_STILL_FRAMES,
} from "./AgentsStill";
import { TOTAL_FRAMES } from "./timing";
import { FPS, WIDTH, HEIGHT } from "./theme";
import "./index.css";

export const RemotionRoot: React.FC = () => {
  return (
    <>
      <Composition
        id="Ad"
        component={Ad}
        durationInFrames={TOTAL_FRAMES}
        fps={FPS}
        width={WIDTH}
        height={HEIGHT}
      />
      {/* Ad B: 44s retention-data variant (agent-memory wedge) */}
      <Composition
        id="AdB"
        component={AdB}
        durationInFrames={TOTAL_FRAMES_B}
        fps={FPS}
        width={WIDTH}
        height={HEIGHT}
      />
      {/* Landing-page board loop: bunx remotion render BoardLoop ../site/public/board-loop.mp4 */}
      <Composition
        id="BoardLoop"
        component={BoardLoop}
        durationInFrames={BOARD_LOOP_FRAMES}
        fps={FPS}
        width={BOARD_LOOP_W}
        height={BOARD_LOOP_H}
      />
      {/* Landing-page plan-sync loop: bunx remotion render PlanSync ../site/public/plan-sync.mp4 */}
      <Composition
        id="PlanSync"
        component={PlanSync}
        durationInFrames={PLAN_SYNC_FRAMES}
        fps={FPS}
        width={PLAN_SYNC_W}
        height={PLAN_SYNC_H}
      />
      {/* Landing-page "For everyone" still: see the freeze prop in EveryoneSplit.tsx for the render and crop commands */}
      <Composition
        id="EveryoneSplit"
        component={EveryoneSplit}
        durationInFrames={EVERYONE_SPLIT_FRAMES}
        fps={FPS}
        width={EVERYONE_SPLIT_W}
        height={EVERYONE_SPLIT_H}
      />
      {/* Landing-page "For agents" still: render and crop commands in AgentsStill.tsx */}
      <Composition
        id="AgentsStill"
        component={AgentsStill}
        durationInFrames={AGENTS_STILL_FRAMES}
        fps={FPS}
        width={AGENTS_STILL_W}
        height={AGENTS_STILL_H}
      />
      {/* README hero image: npx remotion still Hero ../LificHero.png */}
      <Still id="Hero" component={Hero} width={1920} height={1080} />
    </>
  );
};

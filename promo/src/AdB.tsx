import React from "react";
import { Audio, staticFile, interpolate } from "remotion";
import {
  TransitionSeries,
  linearTiming,
  springTiming,
} from "@remotion/transitions";
import { fade } from "@remotion/transitions/fade";
import { slide } from "@remotion/transitions/slide";
import { AgentHook } from "./scenes/AgentHook";
import { ProblemFlash } from "./scenes/ProblemFlash";
import { Reveal } from "./scenes/Reveal";
import { InitScene } from "./scenes/InitScene";
import { WebUIScene } from "./scenes/WebUIScene";
import { PlansScene } from "./scenes/PlansScene";
import { Cta } from "./scenes/Cta";

/*
 * Ad B: the retention-data variant. Changes vs Ad A, driven by the
 * first flight's funnel (LIF-260):
 *   1. product in frame one (demo-first cold open)
 *   2. problem compressed 13s -> 7s
 *   3. `lific init` (2.0) replaces install/start/connect
 *   4. new Plans scene ("survives the context window") on drop 2
 *   5. total 59s -> 44.3s (24 bars @ 130 BPM)
 * Grid: same music.wav; drop 1 (bar 9, frame 443) = reveal cut,
 * drop 2 (bar 17, frame 886) = plans cut. All cut mids on beats.
 */

/*
 * v2 rebalance (caption-readability pass): the web UI scene (list ->
 * board) owns the whole segment between drop 1 and drop 2; init moves
 * AFTER plans as the classic get-started beat before the CTA. Captions
 * everywhere appear early and hold 5s+. Total = 27 bars (49.8s).
 */
export const SCENES_B = {
  agentHook: 248,
  problem: 213,
  reveal: 123,
  webui: 344,
  plans: 234,
  init: 178,
  cta: 227,
} as const;

const T = 12;
const durs = Object.values(SCENES_B);
export const TOTAL_FRAMES_B =
  durs.reduce((a, b) => a + b, 0) - (durs.length - 1) * T;

const cut = linearTiming({ durationInFrames: T });
const springy = springTiming({ config: { damping: 200 }, durationInFrames: T });

export const AdB: React.FC = () => {
  return (
    <>
      <Audio
        src={staticFile("music.wav")}
        volume={(f) =>
          interpolate(f, [TOTAL_FRAMES_B - 90, TOTAL_FRAMES_B - 6], [1, 0], {
            extrapolateLeft: "clamp",
            extrapolateRight: "clamp",
          })
        }
      />
      <TransitionSeries>
        <TransitionSeries.Sequence durationInFrames={SCENES_B.agentHook}>
          <AgentHook />
        </TransitionSeries.Sequence>
        <TransitionSeries.Transition
          presentation={slide({ direction: "from-right" })}
          timing={springy}
        />

        <TransitionSeries.Sequence durationInFrames={SCENES_B.problem}>
          <ProblemFlash />
        </TransitionSeries.Sequence>
        <TransitionSeries.Transition presentation={fade()} timing={cut} />

        <TransitionSeries.Sequence durationInFrames={SCENES_B.reveal}>
          <Reveal />
        </TransitionSeries.Sequence>
        <TransitionSeries.Transition presentation={fade()} timing={cut} />

        <TransitionSeries.Sequence durationInFrames={SCENES_B.webui}>
          <WebUIScene />
        </TransitionSeries.Sequence>
        <TransitionSeries.Transition presentation={fade()} timing={cut} />

        <TransitionSeries.Sequence durationInFrames={SCENES_B.plans}>
          <PlansScene />
        </TransitionSeries.Sequence>
        <TransitionSeries.Transition
          presentation={slide({ direction: "from-bottom" })}
          timing={springy}
        />

        <TransitionSeries.Sequence durationInFrames={SCENES_B.init}>
          <InitScene />
        </TransitionSeries.Sequence>
        <TransitionSeries.Transition presentation={fade()} timing={cut} />

        <TransitionSeries.Sequence durationInFrames={SCENES_B.cta}>
          <Cta />
        </TransitionSeries.Sequence>
      </TransitionSeries>
    </>
  );
};

import React from "react";
import { Audio, staticFile } from "remotion";
import {
  TransitionSeries,
  linearTiming,
  springTiming,
} from "@remotion/transitions";
import { fade } from "@remotion/transitions/fade";
import { slide } from "@remotion/transitions/slide";
import { SCENES, TRANSITION } from "./timing";
import { Hook } from "./scenes/Hook";
import { AgitateJira, AgitateLinear, AgitateFoss } from "./scenes/Agitate";
import { Reveal } from "./scenes/Reveal";
import { TerminalScene } from "./scenes/TerminalScene";
import { UIScene } from "./scenes/UIScene";
import { AgentScene } from "./scenes/AgentScene";
import { TeamsScene } from "./scenes/TeamsScene";
import { Cta } from "./scenes/Cta";

/**
 * music.wav: composed for this cut — 130 BPM, 32 bars, drops on bar 9
 * (the reveal) and bar 17 (the kanban grab). The video is still
 * designed sound-off first (X autoplays muted).
 */
const MUSIC = true;

const cut = linearTiming({ durationInFrames: TRANSITION });
const springy = springTiming({
  config: { damping: 200 },
  durationInFrames: TRANSITION,
});

export const Ad: React.FC = () => {
  return (
    <>
      {MUSIC ? <Audio src={staticFile("music.wav")} /> : null}
      <TransitionSeries>
        <TransitionSeries.Sequence durationInFrames={SCENES.hook}>
          <Hook />
        </TransitionSeries.Sequence>
        <TransitionSeries.Transition
          presentation={slide({ direction: "from-right" })}
          timing={springy}
        />

        <TransitionSeries.Sequence durationInFrames={SCENES.jira}>
          <AgitateJira />
        </TransitionSeries.Sequence>
        <TransitionSeries.Transition
          presentation={slide({ direction: "from-right" })}
          timing={springy}
        />

        <TransitionSeries.Sequence durationInFrames={SCENES.linear}>
          <AgitateLinear />
        </TransitionSeries.Sequence>
        <TransitionSeries.Transition
          presentation={slide({ direction: "from-right" })}
          timing={springy}
        />

        <TransitionSeries.Sequence durationInFrames={SCENES.foss}>
          <AgitateFoss />
        </TransitionSeries.Sequence>
        <TransitionSeries.Transition presentation={fade()} timing={cut} />

        <TransitionSeries.Sequence durationInFrames={SCENES.reveal}>
          <Reveal />
        </TransitionSeries.Sequence>
        <TransitionSeries.Transition presentation={fade()} timing={cut} />

        <TransitionSeries.Sequence durationInFrames={SCENES.terminal}>
          <TerminalScene />
        </TransitionSeries.Sequence>
        <TransitionSeries.Transition
          presentation={slide({ direction: "from-bottom" })}
          timing={springy}
        />

        <TransitionSeries.Sequence durationInFrames={SCENES.ui}>
          <UIScene />
        </TransitionSeries.Sequence>
        <TransitionSeries.Transition presentation={fade()} timing={cut} />

        <TransitionSeries.Sequence durationInFrames={SCENES.agent}>
          <AgentScene />
        </TransitionSeries.Sequence>
        <TransitionSeries.Transition presentation={fade()} timing={cut} />

        <TransitionSeries.Sequence durationInFrames={SCENES.teams}>
          <TeamsScene />
        </TransitionSeries.Sequence>
        <TransitionSeries.Transition presentation={fade()} timing={cut} />

        <TransitionSeries.Sequence durationInFrames={SCENES.cta}>
          <Cta />
        </TransitionSeries.Sequence>
      </TransitionSeries>
    </>
  );
};

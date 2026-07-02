import React from "react";
import { Audio, staticFile } from "remotion";
import { TransitionSeries, linearTiming, springTiming } from "@remotion/transitions";
import { fade } from "@remotion/transitions/fade";
import { slide } from "@remotion/transitions/slide";
import { SCENES, TRANSITION } from "./timing";
import { ColdOpen } from "./scenes/ColdOpen";
import { AgitateJira, AgitateLinear, AgitateFoss } from "./scenes/Agitate";
import { Reveal } from "./scenes/Reveal";
import { TerminalScene } from "./scenes/TerminalScene";
import { UIScene } from "./scenes/UIScene";
import { AgentScene } from "./scenes/AgentScene";
import { Proof } from "./scenes/Proof";
import { TeamsScene } from "./scenes/TeamsScene";
import { Cta } from "./scenes/Cta";

/**
 * Drop a licensed track at promo/public/music.mp3 and flip this on.
 * The video is designed sound-off first (X autoplays muted), so the
 * track is an enhancer, never a dependency.
 */
const MUSIC = false;

const cut = linearTiming({ durationInFrames: TRANSITION });
const springy = springTiming({
  config: { damping: 200 },
  durationInFrames: TRANSITION,
});

export const Ad: React.FC = () => {
  return (
    <>
      {MUSIC ? <Audio src={staticFile("music.mp3")} volume={0.8} /> : null}
      <TransitionSeries>
        <TransitionSeries.Sequence durationInFrames={SCENES.coldOpen}>
          <ColdOpen />
        </TransitionSeries.Sequence>
        <TransitionSeries.Transition presentation={fade()} timing={cut} />

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

        <TransitionSeries.Sequence durationInFrames={SCENES.proof}>
          <Proof />
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

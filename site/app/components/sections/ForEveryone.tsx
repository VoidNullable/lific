/* eslint-disable @next/next/no-img-element */
import { Reveal } from "../Reveal";
import { SectionTitle, Body, Window } from "../landing";

export function ForEveryone() {
  return (
    <section className="band mt-[clamp(8rem,18vh,11rem)] py-[clamp(3.5rem,8vh,5.5rem)]">
      <Reveal>
        <SectionTitle>everyone</SectionTitle>
        <Body className="mt-8">
          Feel less like a prompter and more like a director.
        </Body>
      </Reveal>
      <Reveal delay={100} className="mt-9 min-w-0">
        {/* One frozen moment, three windows: the agent's session, the
            board in the browser, and the project activity that records
            both. Cut from the EveryoneSplit composition in promo/. */}
        <div className="grid min-w-0 gap-4 md:grid-cols-[703fr_1029fr]">
          <Window title="opencode · ~/dev/app" className="min-w-0">
            <img
              src="/everyone-agent.webp"
              width={703}
              height={502}
              loading="lazy"
              alt="An agent session: lific_get_plan, then the reply 'APP-43 is next on the plan. Starting it.', then lific_update_issue setting APP-43 to active"
              className="block h-auto w-full max-md:h-[150px] max-md:object-cover max-md:object-left-top"
            />
          </Window>
          <Window title="localhost:3456/#/APP/board" className="min-w-0">
            <img
              src="/everyone-board.webp"
              width={1029}
              height={502}
              loading="lazy"
              alt="The Lific board: APP-43 now in Active, labelled opencode-blake, and APP-51 just dropped in Done by blake's cursor"
              className="block h-auto w-full"
            />
          </Window>
        </div>
        <Window title="APP · activity" className="mt-4 min-w-0">
          <img
            src="/everyone-activity.webp"
            width={1760}
            height={153}
            loading="lazy"
            alt="Activity: opencode-blake changed status of APP-43 from Todo to Active via mcp; blake changed status of APP-51 from Todo to Done via web"
            className="block h-auto w-full"
          />
        </Window>
        <p className="mt-3 text-caption text-text-faint">
          Two people&apos;s worth of work. One of them is an agent.
        </p>
      </Reveal>
    </section>
    
    
  );
}

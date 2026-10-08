/* eslint-disable @next/next/no-img-element */
import { Reveal } from "../Reveal";
import { SectionTitle, Body, Cmd, Window } from "../landing";

// Three calls an agent makes all day, with what each one returns.
const agentCalls: { key: string; call: string; result: React.ReactNode }[] = [
  {
    key: "workable",
    call: "list_issues(workable=true)",
    result: "Skips APP-44, which waits on @blake.",
  },
  {
    key: "evidence",
    call: 'update_issue(status="done", evidence=...)',
    result:
      "Closes APP-43, ticks plan step #4 and records a verification comment.",
  },
  {
    key: "edit",
    call: "edit_issue(old_string=..., new_string=...)",
    result: "Changes one string in the description without resending the rest.",
  },
];

export function ForAgents() {
  return (
    <section className="mt-[clamp(8rem,18vh,11rem)]">
      <Reveal>
        <div className="flex min-w-0 items-end justify-between gap-8">
          <div className="min-w-0 max-w-4xl">
            <SectionTitle>agents</SectionTitle>
            <Body className="mt-8">Pick up where the last session stopped.</Body>
          </div>
          <img
            src="/LizzyReading.png"
            alt=""
            width={90}
            height={130}
            className="hidden shrink-0 opacity-80 sm:block"
          />
        </div>
      </Reveal>

      <Reveal delay={100} className="mt-9 min-w-0">
        {/* A fresh session's first call. Cut from the AgentsStill
            composition in promo/; the reply restates what get_briefing
            returned for the seeded story. Scrolls sideways on phones so
            the text stays legible. */}
        <Window title="opencode · ~/dev/app" className="min-w-0">
          <div className="overflow-x-auto">
            <img
              src="/agents-session.webp"
              width={1400}
              height={270}
              loading="lazy"
              alt="An agent session on Thursday at 9:01 am: lific_get_briefing for project APP since the last session, then the reply 'Since the last session, APP-42 closed and APP-48 went active. Ship offline sync is 5/7 done. Step 4 is APP-43; APP-44 waits on you. Starting APP-43.'"
              className="block h-auto w-full min-w-[720px]"
            />
          </div>
        </Window>
      </Reveal>

      <Reveal delay={150}>
        <ul className="mt-8 grid divide-y divide-border/60 border-y border-border/60 md:grid-cols-3 md:divide-x md:divide-y-0">
          {agentCalls.map(({ key, call, result }) => (
            <li
              key={key}
              className="min-w-0 py-4 md:px-5 md:first:pl-0 md:last:pr-0"
            >
              <code className="block font-mono text-body-sm leading-relaxed text-text">
                {call}
              </code>
              <p className="mt-1 text-body leading-relaxed text-text-faint">
                {result}
              </p>
            </li>
          ))}
        </ul>
        <Body className="mt-8">
          31 tools in 5,787 tokens of context. <Cmd>lific connect</Cmd> sets
          them up in 11 AI clients.
        </Body>
      </Reveal>
    </section>
  );
}

/* eslint-disable @next/next/no-img-element */
import { Reveal } from "../Reveal";
import { SectionTitle, Body, Window } from "../landing";

// Project roles, as projects.mdx defines them. Each includes the ones below.
const ROLES = [
  {
    name: "Lead",
    line: "Manages members and settings, and decides whether the project is public.",
  },
  {
    name: "Maintainer",
    line: "Creates and edits issues, pages and plans, and checks off plan steps.",
  },
  {
    name: "Viewer",
    line: "Reads everything in the project and can comment.",
  },
];

export function ForTeams() {
  return (
    <section className="mt-[clamp(8rem,18vh,11rem)]">
      <Reveal>
        <SectionTitle>teams</SectionTitle>
        <Body className="mt-8">Every change has a name.</Body>
      </Reveal>

      <Reveal delay={100} className="mt-9 min-w-0">
        {/* One morning on APP: the issue page that records who closed
            APP-42 and how, beside the plan that closing it advanced. Cut
            from the TeamsStill composition in promo/. Both images are 640px
            tall, so the column ratio keeps the windows level. */}
        <div className="grid min-w-0 gap-4 md:grid-cols-[845fr_713fr]">
          <Window title="APP-42 · Conflict resolution" className="min-w-0">
            <img
              src="/teams-activity.webp"
              width={845}
              height={640}
              loading="lazy"
              alt="APP-42 activity, newest first: opencode-blake (agent) commented 'cargo test: 412 passed, 0 failed.', auto-completed a step (issue closed), and changed status from Active to Done, all via mcp; cursor-ada (agent) changed the description via mcp; ada changed priority from Medium to High via web. Below, comment #118 from opencode-blake, marked Verification: cargo test: 412 passed, 0 failed."
              className="block h-auto w-full"
            />
          </Window>
          <Window title="APP-PLAN-2 · Ship offline sync" className="min-w-0">
            <img
              src="/teams-plan.webp"
              width={713}
              height={640}
              loading="lazy"
              alt="Plan APP-PLAN-2, Ship offline sync, 5 of 7 steps done: schema migration (APP-39), write-ahead op queue (APP-40) and conflict resolution (APP-42) with both its sub-steps are checked off; retry with exponential backoff (APP-43) and feature flag and rollout notes are still open"
              className="block h-auto w-full"
            />
          </Window>
        </div>
      </Reveal>

      <Reveal delay={150}>
        <ul className="mt-9 grid max-w-4xl md:grid-cols-3 md:gap-x-8">
          {ROLES.map((r) => (
            <li
              key={r.name}
              className="border-t border-border/60 py-4 text-body leading-relaxed max-md:last:border-b"
            >
              <p className="font-medium text-text">{r.name}</p>
              <p className="mt-0.5 text-text-faint">{r.line}</p>
            </li>
          ))}
        </ul>
        <Body className="mt-6">
          Each AI tool connects as its own account under yours, so you can
          revoke one without touching the others.
        </Body>
      </Reveal>
    </section>
  );
}

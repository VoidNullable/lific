/* eslint-disable @next/next/no-img-element */
import { Reveal } from "../Reveal";
import { AutoplayVideo } from "../AutoplayVideo";
import { SectionTitle, Body, Em, Window } from "../landing";

export function ForTeams() {
  return (
    <section className="mt-[clamp(8rem,18vh,11rem)]">
      <Reveal>
        <SectionTitle>teams</SectionTitle>
        <Body className="mt-8">
          Before an agent writes code, it writes a plan: a tree of steps,
          nested wherever a step needs its own sub-steps. Lific stores
          that tree in the tracker and ties each step to a real issue.{" "}
          <Em>Finishing a step closes its issue</Em>, so the board your
          team watches stays current while the agent works.
        </Body>
      </Reveal>
    
      <Reveal delay={100} className="mt-9 min-w-0 lg:-ml-16">
        <Window
          title="APP-PLAN-2 · Ship offline sync"
          className="min-w-0 w-full max-w-full"
        >
          <AutoplayVideo
            src="/plan-sync.mp4"
            poster="/plan-poster.webp"
            aspect="aspect-[1832/620]"
            label="An agent finishes a plan step and its sub-step; Lific checks them off and closes the linked issue; a later session resumes from the next step"
          />
        </Window>
      </Reveal>
    
      <Reveal delay={150}>
        <Body className="mt-9">
          Planning a quarter is the same act as planning a coding
          session, just a longer tree: steps and sub-steps checked off
          one by one, top to bottom.
        </Body>
        <ul className="mt-8 max-w-4xl">
          <li className="border-t border-border/60 py-4 text-body leading-relaxed">
            <p className="font-medium text-text">Project-scoped roles</p>
            <p className="mt-0.5 text-text-faint">
              Viewer, maintainer, and lead memberships checked on every
              project-scoped REST and MCP call, reads included. Instance
              administrators and operator-trusted credentials intentionally
              bypass project membership checks. Fresh installs enforce this
              out of the box.
            </p>
          </li>
          <li className="border-t border-border/60 py-4 text-body leading-relaxed">
            <p className="font-medium text-text">OAuth 2.1 for connected tools</p>
            <p className="mt-0.5 text-text-faint">
              Connected clients can sign in through a standard flow instead
              of pasted keys, so agent actions land under the right name.
            </p>
          </li>
          <li className="border-t border-border/60 border-b py-4 text-body leading-relaxed">
            <p className="font-medium text-text">
              Comments, @mentions, and an audit trail
            </p>
            <p className="mt-0.5 text-text-faint">
              Humans and agents discuss work in the same threads, and every
              change records who made it and through which door: web, MCP,
              API, or CLI.
            </p>
          </li>
        </ul>
      </Reveal>
    </section>
    
    
  );
}

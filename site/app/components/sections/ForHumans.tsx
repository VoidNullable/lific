/* eslint-disable @next/next/no-img-element */
import { Reveal } from "../Reveal";
import { AutoplayVideo } from "../AutoplayVideo";
import { SectionTitle, Body, Em, Cmd, Window } from "../landing";

export function ForHumans() {
  return (
    <section className="band mt-[clamp(8rem,18vh,11rem)] py-[clamp(3.5rem,8vh,5.5rem)]">
      <Reveal>
        <SectionTitle>humans</SectionTitle>
        <Body className="mt-8">
          Agents work over MCP. Humans get a full web UI in the same
          binary, at <Cmd>localhost:3456</Cmd>: an issue list, a kanban board, documents, modules, and{" "}
          <Em>comment threads where you and your agents talk to each
          other</Em>. Dark mode is the default, with accent presets and a
          light theme in settings.
        </Body>
        <Body>
          It also catches the ideas. File a half-formed thought as a
          backlog issue from your phone, and it&apos;s still sitting there
          next week when an agent asks for work.
        </Body>
      </Reveal>
      <Reveal delay={100} className="mt-9 lg:-mr-16">
        <Window title="localhost:3456/#/APP/board">
          <AutoplayVideo
            src="/board-loop.mp4"
            poster="/board-poster.webp"
            aspect="aspect-[1832/860]"
            label="The Lific kanban board moving an issue from todo through active to done"
          />
        </Window>
      </Reveal>
    </section>
    
    
  );
}

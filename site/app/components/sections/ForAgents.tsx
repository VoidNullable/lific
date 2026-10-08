/* eslint-disable @next/next/no-img-element */
import { Reveal } from "../Reveal";
import { SectionTitle, Body, Em, Cmd, Window } from "../landing";

const agentFacts: {
  key: string;
  head: React.ReactNode;
  body: React.ReactNode;
}[] = [
  {
    key: "workable",
    head: <>&quot;What can I work on?&quot; in one call</>,
    body: (
      <>
        The <Cmd>workable</Cmd> filter returns only issues with every
        blocker resolved, so triage happens without a graph query.
      </>
    ),
  },
  {
    key: "agents-md",
    head: <Cmd>lific agents-md</Cmd>,
    body: (
      <>
        Generates tracker instructions for your repo&apos;s AGENTS.md, so a
        fresh session knows where the work lives before it reads a single
        file.
      </>
    ),
  },
  {
    key: "identifiers",
    head: <>Identifiers that survive a prompt</>,
    body: (
      <>
        Everything gets a name like{" "}
        <span className="identifier-link">APP-42</span> that holds up in a
        log, a grep, a commit message, or a conversation.
      </>
    ),
  },
];


export function ForAgents() {
  return (
    <section className="mt-[clamp(8rem,18vh,11rem)]">
      <Reveal>
        <div className="flex min-w-0 items-end justify-between gap-8">
          <div className="min-w-0 max-w-4xl">
            <SectionTitle>agents</SectionTitle>
            <Body className="mt-8">
              <Cmd>lific connect</Cmd> detects the AI tools installed on
              your machine and writes the MCP config for each one you
              pick:
            </Body>
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
    
      {/* lific connect, as it actually renders */}
      <Reveal delay={100}>
        <Window
          title="~/dev/app"
          className="mt-9 min-w-0 w-full max-w-full md:max-w-4xl"
        >
          <pre className="min-w-0 max-w-full overflow-x-auto whitespace-pre bg-bg p-4 font-mono text-body-sm leading-loose text-text-muted sm:p-6">
            <code>
              <span className="text-success">$</span>{" "}
              <span className="text-text">lific connect</span>
              {"\n"}
              <span className="text-text-faint">&#9484;</span>
              {"  lific connect\n"}
              <span className="text-accent">&#9670;</span>
              {"  Which clients should connect to http://localhost:3456?\n"}
              <span className="text-text-faint">&#9474;</span>
              {"  "}
              <span className="text-accent">&#9724;</span>{" "}
              <span className="text-text">OpenCode</span>
              {"      "}
              <span className="text-text-faint">detected</span>
              {"\n"}
              <span className="text-text-faint">&#9474;</span>
              {"  "}
              <span className="text-accent">&#9724;</span>{" "}
              <span className="text-text">Claude Code</span>
              {"   "}
              <span className="text-text-faint">detected</span>
              {"\n"}
              <span className="text-text-faint">
                &#9474; &#9723; Cursor
              </span>
              {"\n"}
              <span className="text-text-faint">&#9474; &#9723; Zed</span>
              {"\n"}
              <span className="text-success">&#9671;</span>
              {"  Claude Code "}
              <span className="text-text-faint">
                &mdash; updated ~/.claude.json
              </span>
              {"\n"}
              <span className="text-success">&#9671;</span>
              {"  OpenCode "}
              <span className="text-text-faint">
                &mdash; updated ~/.config/opencode/opencode.json
              </span>
              {"\n"}
              <span className="text-text-faint">&#9492;</span>
              {"  Restart your client(s) to pick up the new MCP server."}
            </code>
          </pre>
        </Window>
      </Reveal>
    
      <Reveal delay={150}>
        <Body className="mt-9">
          After the restart, the agent has the whole tracker as MCP tools:
          issues, plans, pages, comments, and search. The full tool
          surface costs <Em>about 5.8k tokens of context</Em>, roughly one
          long file read, so it leaves room for the actual work.
        </Body>
        <Body>
          Agents without MCP support get the same verbs through the CLI.
          Data commands automatically <Em>emit JSON</Em> when stdout is not
          a terminal, and <Cmd>lific doctor</Cmd> exits nonzero when the
          setup is broken. The tracker fits into scripts and CI as
          comfortably as into conversations.
        </Body>
        <ul className="mt-8 max-w-4xl">
          {agentFacts.map(({ key, head, body }) => (
            <li
              key={key}
              className="border-t border-border/60 py-4 text-body leading-relaxed last:border-b"
            >
              <p className="font-medium text-text">{head}</p>
              <p className="mt-0.5 text-text-faint">{body}</p>
            </li>
          ))}
        </ul>
      </Reveal>
    </section>
    
    
  );
}

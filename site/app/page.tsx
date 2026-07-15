/* eslint-disable @next/next/no-img-element */
import type { Metadata } from "next";
import { CopyButton } from "./components/CopyButton";
import { StarCount } from "./components/StarCount";
import { Reveal } from "./components/Reveal";
import { AutoplayVideo } from "./components/AutoplayVideo";
import { VersionChip } from "./components/VersionChip";

const GITHUB = "https://github.com/VoidNullable/lific";
const CRATE = "https://crates.io/crates/lific";
const DISCORD = "https://discord.gg/uWvaFC4f7D";
const RELEASES = "https://github.com/VoidNullable/lific/releases";

export const metadata: Metadata = {
  alternates: { canonical: "/" },
};

// Structured data: tells crawlers this is a free developer application
// and ties the site to its GitHub/crates.io/Discord identities.
const JSONLD = JSON.stringify([
  {
    "@context": "https://schema.org",
    "@type": "SoftwareApplication",
    name: "Lific",
    url: "https://lific.dev",
    description:
      "A free, self-hosted issue tracker built for coding agents. Single binary, native MCP.",
    applicationCategory: "DeveloperApplication",
    // Native Windows is supported via `cargo install` (docs/windows);
    // prebuilt release binaries are Linux + macOS only.
    operatingSystem: "Linux, macOS, Windows",
    license: "https://www.apache.org/licenses/LICENSE-2.0",
    offers: { "@type": "Offer", price: "0", priceCurrency: "USD" },
    sameAs: [GITHUB, CRATE, DISCORD],
  },
  {
    "@context": "https://schema.org",
    "@type": "WebSite",
    name: "Lific",
    url: "https://lific.dev",
  },
]);


// The section-label hallmark pattern (LIF-DOC-14 §2). Used only for
// the hero eyebrow; the pitch sections carry their own big titles.
function SectionLabel({ children }: { children: React.ReactNode }) {
  return (
    <p className="text-micro font-semibold uppercase tracking-widest text-text-faint">
      {children}
    </p>
  );
}

// The four-beat pitch title: "For <audience>" IS the heading.
function SectionTitle({ children }: { children: React.ReactNode }) {
  return (
    <h2 className="font-display text-[clamp(2.25rem,5vw,3.5rem)] font-semibold leading-none tracking-tight">
      <span className="text-text-faint">For</span> {children}
    </h2>
  );
}

// Landing-prose paragraph: bigger than app body, muted, capped measure.
function Body({
  children,
  className = "",
}: {
  children: React.ReactNode;
  className?: string;
}) {
  return (
    <p
      className={`mt-4 max-w-[62ch] text-lead leading-relaxed text-text-muted ${className}`}
    >
      {children}
    </p>
  );
}

// A phrase worth catching while skimming.
function Em({ children }: { children: React.ReactNode }) {
  return <span className="font-medium text-text">{children}</span>;
}

// Inline command chip, the product's .prose code recipe (app.css):
// mono, bordered, on the focal surface tier so it reads on both the
// page floor and the section bands.
function Cmd({ children }: { children: React.ReactNode }) {
  return (
    <code className="whitespace-nowrap rounded-[4px] border border-border bg-surface px-[0.4em] py-[0.15em] font-mono text-[0.8125em] text-text">
      {children}
    </code>
  );
}

// Shared window chrome for every media artifact on the page: videos
// and terminals live in the same shell, the way they would on a desk.
function Window({
  title,
  children,
  className = "",
}: {
  title: string;
  children: React.ReactNode;
  className?: string;
}) {
  return (
    <div
      className={`overflow-hidden rounded-xl border border-border bg-chrome shadow-[0_24px_60px_-24px_rgb(0_0_0/0.6)] ${className}`}
    >
      <div className="flex h-9 items-center gap-1.5 border-b border-border px-4">
        <span aria-hidden className="size-2.5 rounded-full bg-border" />
        <span aria-hidden className="size-2.5 rounded-full bg-border" />
        <span aria-hidden className="size-2.5 rounded-full bg-border" />
        <span className="flex-1 text-center font-mono text-micro text-text-faint">
          {title}
        </span>
        <span aria-hidden className="w-[54px]" />
      </div>
      {children}
    </div>
  );
}

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

export default function Home() {
  return (
    <div className="flex-1">
      <script
        type="application/ld+json"
        dangerouslySetInnerHTML={{ __html: JSONLD }}
      />
      {/* Sticky chrome bar, borrowed from the product's brand header
          (Layout.svelte): logo + font-display wordmark + mono version
          chip on --chrome. */}
      <header className="sticky top-3 z-30 mx-auto w-full max-w-5xl px-4 sm:px-6">
        <div className="flex items-center gap-2.5 rounded-xl border border-border bg-chrome px-3 py-2 shadow-lg">
          <a
            href={GITHUB}
            className="group flex min-w-0 items-center gap-2.5 rounded-lg px-1 py-1 transition-colors hover:bg-bg-subtle focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent"
            title="View Lific on GitHub"
          >
            <img
              src="/logo.webp"
              alt=""
              width={26}
              height={26}
              className="shrink-0 rounded-md"
            />
            <span className="font-display text-heading leading-none tracking-tight text-text">
              Lific
            </span>
            <VersionChip />
          </a>
          <div className="flex-1" />
          <nav aria-label="Primary" className="flex items-center gap-1">
            <a
              className="flex h-7 items-center rounded-md px-2 text-caption font-medium text-text-muted transition-colors hover:bg-bg-subtle hover:text-text focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent"
              href="/docs"
            >
              Docs
            </a>
            <a
              className="flex h-7 items-center rounded-md px-2 text-caption font-medium text-text-muted transition-colors hover:bg-bg-subtle hover:text-text focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent"
              href="/compare"
            >
              Compare
            </a>
            <a
              className="hidden h-7 items-center rounded-md px-2 text-caption font-medium text-text-muted transition-colors hover:bg-bg-subtle hover:text-text sm:flex focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent"
              href={CRATE}
            >
              crates.io
            </a>
            <a
              className="hidden h-7 items-center rounded-md px-2 text-caption font-medium text-text-muted transition-colors hover:bg-bg-subtle hover:text-text sm:flex focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent"
              href={DISCORD}
            >
              Discord
            </a>
            <a
              className="flex h-7 items-center rounded-md px-2 text-caption font-medium text-text-muted transition-colors hover:bg-bg-subtle hover:text-text focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent"
              href={GITHUB}
            >
              GitHub
              <StarCount />
            </a>
            {/* Green primary CTA. Indigo is the accent, green is the
                action color (LIF-DOC-14 §3). */}
            <a
              className="ml-1 rounded-md bg-btn-success px-3 py-1.5 text-body-sm font-medium text-btn-success-text transition-colors hover:bg-btn-success-hover motion-safe:active:scale-[0.97] motion-safe:transition-transform focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent"
              href="#install"
            >
              Install
            </a>
          </nav>
        </div>
      </header>

      <main className="mx-auto w-full max-w-5xl px-6">
        {/* Hero */}
        <section className="pt-[clamp(4rem,12vh,7rem)] text-center">
          <div className="animate-reveal flex justify-center">
            <SectionLabel>
              Native MCP · one binary · self-hosted · free &amp; open source
            </SectionLabel>
          </div>
          <h1 className="animate-reveal delay-100 mx-auto mt-5 font-display text-[clamp(2.75rem,7vw,4.75rem)] font-semibold leading-[1.08] tracking-tight">
            An issue tracker
            <br />
            <span className="brand-gradient-text">for prolific agents.</span>
          </h1>
          <p className="animate-reveal delay-200 mx-auto mt-7 max-w-[56ch] text-body-lg leading-relaxed text-text-muted md:text-heading md:font-normal">
            Built for coding agents. Plans and issues live on your server
            instead of the context window, so work outlives the session.
          </p>

          {/* The install commands, front and center */}
          <div
            id="install"
            className="animate-reveal delay-300 mx-auto mt-12 max-w-xl scroll-mt-24"
          >
            <div className="flex items-center justify-between gap-4 rounded-lg border border-border bg-surface py-4 pl-6 pr-3 text-left shadow-lg">
              <code className="overflow-x-auto whitespace-pre font-mono text-[clamp(0.95rem,2vw,1.15rem)] leading-relaxed">
                <span className="select-none text-success">$ </span>cargo
                install lific{"\n"}
                <span className="select-none text-success">$ </span>lific init
              </code>
              <CopyButton text="cargo install lific && lific init" />
            </div>
            <p className="mt-3 text-caption text-text-faint">
              or grab a static binary from{" "}
              <a
                className="text-text-muted underline decoration-border underline-offset-4 transition-colors hover:text-accent hover:decoration-accent"
                href={RELEASES}
              >
                the releases page
              </a>{" "}
              (Linux and macOS, x86_64 and arm64)
            </p>
          </div>
        </section>

        {/* For agents */}
        <section className="mt-[clamp(8rem,18vh,11rem)]">
          <Reveal>
            <div className="flex items-end justify-between gap-8">
              <div className="max-w-4xl">
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
            <Window title="~/dev/app" className="mt-9 max-w-4xl">
              <pre className="overflow-x-auto bg-bg p-6 font-mono text-body-sm leading-loose text-text-muted">
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
              surface costs <Em>about 6.1k tokens of context</Em>, roughly one
              long file read, so it leaves room for the actual work.
            </Body>
            <Body>
              Agents without MCP support get the same verbs through the CLI.
              Every command <Em>prints JSON</Em> whenever stdout leaves the
              terminal, and <Cmd>lific doctor</Cmd> exits nonzero when the setup
              is broken. The tracker fits into scripts and CI as comfortably as
              into conversations.
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

        {/* For humans */}
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

        {/* For teams */}
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

          <Reveal delay={100} className="mt-9 lg:-ml-16">
            <Window title="APP-PLAN-2 · Ship offline sync">
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
                  Viewer, maintainer, and lead memberships checked on every REST
                  and MCP call, reads included. Fresh installs enforce this out
                  of the box.
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

        {/* For everyone */}
        <section className="band mt-[clamp(8rem,18vh,11rem)] py-[clamp(3.5rem,8vh,5.5rem)]">
          <Reveal>
            <SectionTitle>everyone</SectionTitle>
            <Body className="mt-8">
              Setup takes about a minute. <Cmd>lific init</Cmd> writes the
              config, creates the database,
              and prints your API key once. It also registers a background
              service, so <Em>the server is still running tomorrow</Em>.{" "}
              <Cmd>lific connect</Cmd> finds the AI tools on your machine and
              writes their MCP config for them. Restart your client and the
              tools show up.
            </Body>
            <Body>
              Running solo, that&apos;s the whole ceremony. Keys minted from
              your own shell are <Em>operator-trusted</Em>, and you never make
              an account.
            </Body>
          </Reveal>
          <Reveal delay={100}>
            <Window title="~" className="mt-9 max-w-4xl">
              <pre className="overflow-x-auto bg-bg p-6 font-mono text-body-sm leading-loose text-text">
                <code>
                  <span className="text-success">$</span> cargo install lific
                  {"\n"}
                  <span className="text-success">$</span> lific init
                  {"      "}
                  <span className="text-text-faint">
                    # config + db + api key; service on :3456
                  </span>
                  {"\n"}
                  <span className="text-success">$</span> lific connect
                  {"   "}
                  <span className="text-text-faint">
                    # writes MCP config into your AI clients
                  </span>
                  {"\n"}
                  <span className="text-success">$</span> lific doctor
                  {"    "}
                  <span className="text-text-faint">
                    # health checks; exits nonzero if broken
                  </span>
                </code>
              </pre>
            </Window>
          </Reveal>
        </section>

        {/* Closing */}
        <section className="band band-finale">
          <div className="flex flex-col items-start gap-8 py-[clamp(3rem,7vh,4.5rem)] sm:flex-row sm:items-center sm:justify-between">
            <div>
              <p className="max-w-[28ch] font-display text-[clamp(1.75rem,4vw,2.75rem)] font-semibold leading-tight tracking-tight">
                Issue trackers should be simple,{" "}
                <span className="brand-gradient-text">right?</span>
              </p>
              <div className="mt-8 flex flex-wrap items-center gap-4">
                <a
                  href={GITHUB}
                  className="rounded-md bg-btn-success px-4 py-2.5 text-body-lg font-medium text-btn-success-text transition-colors hover:bg-btn-success-hover motion-safe:active:scale-[0.97] motion-safe:transition-transform"
                >
                  Star on GitHub
                </a>
                <code className="rounded-md border border-border px-3 py-2 font-mono text-body-sm text-text-muted">
                  <span className="select-none text-success">$ </span>cargo
                  install lific
                </code>
              </div>
            </div>
          </div>
        </section>
      </main>

      <footer>
        <div className="mx-auto flex w-full max-w-5xl flex-wrap items-center justify-between gap-4 px-6 py-8 font-mono text-caption text-text-faint">
          <span className="flex items-center gap-2">
            <img
              src="/logo.webp"
              alt=""
              width={16}
              height={16}
              className="rounded"
            />
            © 2026 · Apache-2.0 · no telemetry
          </span>
          <div className="flex gap-5">
            <a className="transition-colors hover:text-text" href={GITHUB}>
              github
            </a>
            <a className="transition-colors hover:text-text" href={CRATE}>
              crates.io
            </a>
            <a className="transition-colors hover:text-text" href={DISCORD}>
              discord
            </a>
          </div>
        </div>
      </footer>
    </div>
  );
}

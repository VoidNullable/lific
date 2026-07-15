/* eslint-disable @next/next/no-img-element */
import type { Metadata } from "next";
import { StarCount } from "../components/StarCount";
import { VersionChip } from "../components/VersionChip";

const GITHUB = "https://github.com/VoidNullable/lific";
const CRATE = "https://crates.io/crates/lific";
const DISCORD = "https://discord.gg/uWvaFC4f7D";
const ISSUES = "https://github.com/VoidNullable/lific/issues";

// The snapshot date. Bump this whenever a cell is re-verified or
// corrected — the page is a dated snapshot, not an evergreen claim.
const STAMP = "July 14, 2026";
const STAMP_ISO = "2026-07-14";

export const metadata: Metadata = {
  title: "Issue trackers with MCP support, compared · Lific",
  description:
    "Lific vs beads, Vikunja, Gitea, Plane, and Linear on MCP support, transports, deployment, storage, and license — including where Lific loses. A date-stamped, sourced comparison.",
  alternates: { canonical: "/compare" },
  openGraph: {
    title: "Issue trackers with MCP support, compared",
    description:
      "Lific vs beads, Vikunja, Gitea, Plane, and Linear — real tables, real losses, and a literal 'when to use something else' section.",
    url: "https://lific.dev/compare",
    type: "article",
  },
};

// Structured data: a dated TechArticle, so crawlers treat this as a
// snapshot comparison with a real publication date.
const JSONLD = JSON.stringify({
  "@context": "https://schema.org",
  "@type": "TechArticle",
  headline: "Issue trackers with MCP support, compared",
  description:
    "A comparison of issue trackers usable by coding agents over the Model Context Protocol: Lific, beads, Vikunja, Gitea, Plane, and Linear.",
  datePublished: STAMP_ISO,
  dateModified: STAMP_ISO,
  url: "https://lific.dev/compare",
  author: { "@type": "Organization", name: "Lific", url: "https://lific.dev" },
});

// Inline command chip, same recipe as the homepage.
function Cmd({ children }: { children: React.ReactNode }) {
  return (
    <code className="whitespace-nowrap rounded-[4px] border border-border bg-surface px-[0.4em] py-[0.15em] font-mono text-[0.8125em] text-text">
      {children}
    </code>
  );
}

// External link, understated: readable in a cell, obvious on hover.
function Ext({ href, children }: { href: string; children: React.ReactNode }) {
  return (
    <a
      href={href}
      rel="noopener"
      className="text-text underline decoration-border underline-offset-4 transition-colors hover:text-accent hover:decoration-accent"
    >
      {children}
    </a>
  );
}

/*
 * Real <table> markup on purpose. Extractors, LLMs, and reader modes
 * eat <table>/<thead>/<th scope> structure; they choke on styled-div
 * grids. Do not "upgrade" these to flex layouts.
 */
const th =
  "border-b border-border px-4 py-3 text-left align-bottom font-display text-body font-semibold text-text";
const td =
  "border-b border-border/60 px-4 py-3.5 align-top text-body-sm leading-relaxed text-text-muted";
const tdName =
  "border-b border-border/60 px-4 py-3.5 align-top font-display text-body font-semibold text-text whitespace-nowrap";

function ComparisonTable({
  caption,
  head,
  rows,
}: {
  caption: string;
  head: string[];
  rows: { name: string; lific?: boolean; cells: React.ReactNode[] }[];
}) {
  return (
    <div className="mt-8 overflow-x-auto rounded-xl border border-border bg-bg-subtle/40">
      <table className="w-full min-w-[820px] border-collapse">
        <caption className="sr-only">{caption}</caption>
        <thead>
          <tr>
            <th scope="col" className={th}>
              Tracker
            </th>
            {head.map((h) => (
              <th scope="col" key={h} className={th}>
                {h}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {rows.map(({ name, lific, cells }) => (
            <tr key={name} className={lific ? "bg-accent-subtle/40" : ""}>
              <th scope="row" className={tdName}>
                {name}
              </th>
              {cells.map((cell, i) => (
                <td key={i} className={td}>
                  {cell}
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function H2({ id, children }: { id?: string; children: React.ReactNode }) {
  return (
    <h2
      id={id}
      className="scroll-mt-24 font-display text-[clamp(1.75rem,4vw,2.5rem)] font-semibold leading-tight tracking-tight"
    >
      {children}
    </h2>
  );
}

function Body({
  children,
  className = "",
}: {
  children: React.ReactNode;
  className?: string;
}) {
  return (
    <p
      className={`mt-4 max-w-[68ch] text-lead leading-relaxed text-text-muted ${className}`}
    >
      {children}
    </p>
  );
}

// A fact list where the lead phrase is the claim and the rest is the
// evidence. Used for both the losses and the wins, same shape — the
// symmetry is the point.
function FactList({
  items,
}: {
  items: { head: React.ReactNode; body: React.ReactNode }[];
}) {
  return (
    <ul className="mt-8 max-w-4xl">
      {items.map(({ head, body }, i) => (
        <li
          key={i}
          className="border-t border-border/60 py-4 text-body leading-relaxed last:border-b"
        >
          <p className="font-medium text-text">{head}</p>
          <p className="mt-0.5 max-w-[75ch] text-text-faint">{body}</p>
        </li>
      ))}
    </ul>
  );
}

function AltSection({
  id,
  title,
  children,
}: {
  id: string;
  title: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <section id={id} className="scroll-mt-24 border-t border-border/60 py-6">
      <h3 className="font-display text-title font-semibold tracking-tight">
        {title}
      </h3>
      <p className="mt-2 max-w-[75ch] text-body leading-relaxed text-text-muted">
        {children}
      </p>
    </section>
  );
}

export default function Compare() {
  return (
    <div className="flex-1">
      <script
        type="application/ld+json"
        dangerouslySetInnerHTML={{ __html: JSONLD }}
      />

      {/* Same sticky chrome bar as the homepage. */}
      <header className="sticky top-3 z-30 mx-auto w-full max-w-5xl px-4 sm:px-6">
        <div className="flex items-center gap-2.5 rounded-xl border border-border bg-chrome px-3 py-2 shadow-lg">
          <a
            href="/"
            className="group flex min-w-0 items-center gap-2.5 rounded-lg px-1 py-1 transition-colors hover:bg-bg-subtle focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent"
            title="Lific home"
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
              className="flex h-7 items-center rounded-md bg-bg-subtle px-2 text-caption font-medium text-text focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent"
              href="/compare"
              aria-current="page"
            >
              Compare
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
            <a
              className="ml-1 rounded-md bg-btn-success px-3 py-1.5 text-body-sm font-medium text-btn-success-text transition-colors hover:bg-btn-success-hover motion-safe:active:scale-[0.97] motion-safe:transition-transform focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent"
              href="/#install"
            >
              Install
            </a>
          </nav>
        </div>
      </header>

      <main className="mx-auto w-full max-w-5xl px-6 pb-24">
        {/* Title */}
        <section className="pt-[clamp(3.5rem,10vh,6rem)]">
          <p className="text-micro font-semibold uppercase tracking-widest text-text-faint">
            Comparison · accurate at time of writing, {STAMP}
          </p>
          <h1 className="mt-5 max-w-[22ch] font-display text-[clamp(2.25rem,5.5vw,3.75rem)] font-semibold leading-[1.08] tracking-tight">
            Issue trackers with MCP support,{" "}
            <span className="brand-gradient-text">compared.</span>
          </h1>
          <Body>
            If you want your coding agent to work against a real issue tracker
            over the Model Context Protocol, these are the options worth
            knowing about: <Ext href={GITHUB}>Lific</Ext>,{" "}
            <Ext href="https://github.com/steveyegge/beads">beads</Ext>,{" "}
            <Ext href="https://vikunja.io">Vikunja</Ext>,{" "}
            <Ext href="https://about.gitea.com">Gitea</Ext>,{" "}
            <Ext href="https://plane.so">Plane</Ext>, and{" "}
            <Ext href="https://linear.app">Linear</Ext>.
          </Body>
          <Body>
            We build Lific, so read accordingly. To keep the table worth
            quoting anyway: every cell is checkable against the linked primary
            sources, the products we compare against are good at what they do,
            and Lific&apos;s losses are listed first.
          </Body>
        </section>

        {/* Losses first. This ordering is deliberate: these facts will be
            surfaced whether or not we state them, so we state them. */}
        <section className="mt-[clamp(4.5rem,10vh,7rem)]">
          <H2 id="losses">Where Lific loses</H2>
          <FactList
            items={[
              {
                head: "No prebuilt Windows binary.",
                body: (
                  <>
                    Releases cover Linux and macOS (x86_64 and arm64). Windows{" "}
                    <a
                      href="/docs/windows"
                      className="text-text underline decoration-border underline-offset-4 hover:text-accent hover:decoration-accent"
                    >
                      works natively
                    </a>
                    , but you compile it yourself with{" "}
                    <Cmd>cargo install lific</Cmd> — and there&apos;s no
                    Windows service integration, so keeping the server running
                    is on you (Task Scheduler or a terminal).
                  </>
                ),
              },
              {
                head: "No repo-local mode.",
                body: (
                  <>
                    Issues live in a server database, not in your repository.
                    If you want the tracker versioned with the code — branching
                    with it, merging with it, readable offline in the checkout
                    — <a href="#beads" className="text-text underline decoration-border underline-offset-4 hover:text-accent hover:decoration-accent">beads does exactly that</a>{" "}
                    and Lific does not.
                  </>
                ),
              },
              {
                head: "A server is required.",
                body: (
                  <>
                    Lific is an always-on service. If the process is down, so
                    is the tracker. beads needs no server at all; Linear runs
                    someone else&apos;s.
                  </>
                ),
              },
              {
                head: "Single maintainer.",
                body: <>The bus factor is one. That is a real risk and you should price it in.</>,
              },
              {
                head: "Young.",
                body: (
                  <>
                    First release April 2026. Short track record, small
                    community. Weight that honestly against tools that have
                    shipped for years.
                  </>
                ),
              },
              {
                head: "Sized for solo developers and small teams.",
                body: (
                  <>
                    SQLite on one box, project-scoped roles, and that&apos;s
                    the ceiling. If you need SSO, org hierarchies, or
                    enterprise scale, Plane and Linear are built for that and
                    Lific is not.
                  </>
                ),
              },
              {
                head: "No hosted option.",
                body: <>There is no cloud to sign up for. You run it, or it doesn&apos;t exist.</>,
              },
            ]}
          />
        </section>

        {/* Table 1: the MCP story */}
        <section className="mt-[clamp(4.5rem,10vh,7rem)]">
          <H2 id="mcp">The MCP story</H2>
          <Body>
            Who ships the MCP server, how it connects, and what an agent
            actually gets once it&apos;s wired up.
          </Body>
          <ComparisonTable
            caption="MCP support across issue trackers"
            head={["MCP server", "Transports", "What agents get"]}
            rows={[
              {
                name: "Lific",
                lific: true,
                cells: [
                  <>Built into the tracker binary. Nothing extra to run.</>,
                  <>
                    Streamable HTTP (OAuth 2.1 or API key), or stdio via{" "}
                    <Cmd>lific mcp</Cmd>
                  </>,
                  <>
                    29 tools: issues, nestable plans, pages, comments, search,
                    audit history. The whole surface costs about 6.1k tokens of
                    context.
                  </>,
                ],
              },
              {
                name: "beads",
                cells: [
                  <>
                    First-party:{" "}
                    <Ext href="https://pypi.org/project/beads-mcp/">
                      beads-mcp
                    </Ext>{" "}
                    on PyPI, wrapping the <Cmd>bd</Cmd> CLI.
                  </>,
                  <>stdio</>,
                  <>
                    Dependency-aware issue graph, ready-work detection,
                    persistent agent memory. beads&apos; own docs recommend the
                    CLI over MCP when the agent has a shell — it costs fewer
                    tokens.
                  </>,
                ],
              },
              {
                name: "Vikunja",
                cells: [
                  <>
                    None first-party. Several community servers of varying
                    scope and upkeep.
                  </>,
                  <>Varies by server (stdio or HTTP)</>,
                  <>
                    Tasks, projects, labels, kanban — through whichever
                    community server you pick and vet yourself.
                  </>,
                ],
              },
              {
                name: "Gitea",
                cells: [
                  <>
                    First-party:{" "}
                    <Ext href="https://gitea.com/gitea/gitea-mcp">
                      gitea-mcp
                    </Ext>
                    , a separate binary you run alongside your instance.
                  </>,
                  <>stdio, HTTP, SSE</>,
                  <>
                    Forge-level tools. Issues get CRUD, comments, and edits;
                    most of the surface is repositories, files, and pull
                    requests.
                  </>,
                ],
              },
              {
                name: "Plane",
                cells: [
                  <>
                    First-party. Hosted at{" "}
                    <Ext href="https://developers.plane.so/dev-tools/mcp-server">
                      mcp.plane.so
                    </Ext>
                    , or run it yourself.
                  </>,
                  <>Streamable HTTP (OAuth 2.1 or PAT), stdio, SSE (legacy)</>,
                  <>
                    Plane&apos;s full API surface: work items, sprints,
                    modules, docs, time tracking.
                  </>,
                ],
              },
              {
                name: "Linear",
                cells: [
                  <>
                    First-party,{" "}
                    <Ext href="https://linear.app/docs/mcp">
                      hosted by Linear
                    </Ext>
                    . Nothing to install — and nothing to self-host.
                  </>,
                  <>Streamable HTTP (OAuth 2.1)</>,
                  <>
                    A curated tool set over Linear&apos;s API: issues,
                    projects, comments, documents. Requires a Linear account.
                  </>,
                ],
              },
            ]}
          />
        </section>

        {/* Table 2: running it */}
        <section className="mt-[clamp(4.5rem,10vh,7rem)]">
          <H2 id="running-it">Running it</H2>
          <Body>
            What you deploy, where the data lives, and under what terms.
          </Body>
          <ComparisonTable
            caption="Deployment, storage, hosting, and license across issue trackers"
            head={["Deployment", "Storage", "Hosted option", "License"]}
            rows={[
              {
                name: "Lific",
                lific: true,
                cells: [
                  <>
                    One Rust binary: <Cmd>cargo install lific</Cmd>, or static
                    binaries for Linux and macOS. Windows is cargo-only — no
                    prebuilt binary.
                  </>,
                  <>SQLite</>,
                  <>None — self-host only</>,
                  <>Apache-2.0</>,
                ],
              },
              {
                name: "beads",
                cells: [
                  <>
                    A CLI with an embedded Dolt database inside your repo
                    (<Cmd>.beads/</Cmd>). No server process.
                  </>,
                  <>Dolt (versioned SQL)</>,
                  <>None — local by design</>,
                  <>MIT</>,
                ],
              },
              {
                name: "Vikunja",
                cells: [
                  <>Single Go binary, or Docker.</>,
                  <>SQLite, MySQL, or PostgreSQL</>,
                  <>Vikunja Cloud</>,
                  <>AGPL-3.0</>,
                ],
              },
              {
                name: "Gitea",
                cells: [
                  <>
                    Single Go binary, or Docker — a whole code forge, not just
                    a tracker.
                  </>,
                  <>SQLite, MySQL, or PostgreSQL</>,
                  <>Gitea Cloud</>,
                  <>MIT</>,
                ],
              },
              {
                name: "Plane",
                cells: [
                  <>
                    Docker or Kubernetes; a multi-service stack (PostgreSQL,
                    Redis, and more).
                  </>,
                  <>PostgreSQL</>,
                  <>Plane Cloud</>,
                  <>AGPL-3.0</>,
                ],
              },
              {
                name: "Linear",
                cells: [
                  <>Nothing to run — it&apos;s SaaS.</>,
                  <>Managed for you</>,
                  <>Hosted only</>,
                  <>Proprietary</>,
                ],
              },
            ]}
          />
        </section>

        {/* Wins, after the losses and the evidence. */}
        <section className="mt-[clamp(4.5rem,10vh,7rem)]">
          <H2 id="wins">Where Lific wins</H2>
          <FactList
            items={[
              {
                head: "The tracker is the MCP server.",
                body: (
                  <>
                    Not a wrapper, bridge, or sidecar. The same binary that
                    stores your issues speaks MCP, so the tools and the data
                    can&apos;t drift apart, and there is no second process to
                    install, version, or babysit.
                  </>
                ),
              },
              {
                head: "Verbs shaped for agent loops.",
                body: (
                  <>
                    <Cmd>workable</Cmd> returns issues with no unresolved
                    blockers in one call. <Cmd>edit_issue</Cmd> patches by
                    exact string replacement instead of resending whole
                    descriptions. Plans are nestable step trees that mirror
                    issues and survive across sessions and context compaction.
                  </>
                ),
              },
              {
                head: "A small context bill.",
                body: (
                  <>
                    About 6.1k tokens for the full 29-tool surface — roughly
                    one long file read — so connecting the tracker doesn&apos;t
                    crowd out the actual work.
                  </>
                ),
              },
              {
                head: "Humans get a real UI in the same binary.",
                body: (
                  <>
                    Issue list, kanban board, documents, and comment threads
                    where you and your agents talk to each other — at{" "}
                    <Cmd>localhost:3456</Cmd>, no extra deployment.
                  </>
                ),
              },
              {
                head: "An audit trail that names the door.",
                body: (
                  <>
                    Every change records who made it and whether it came
                    through the web UI, MCP, the REST API, or the CLI. When an
                    agent goes off script, you can see exactly what it did.
                  </>
                ),
              },
              {
                head: "Setup is one minute, not one afternoon.",
                body: (
                  <>
                    <Cmd>lific init</Cmd> writes config, creates the database,
                    registers a background service. <Cmd>lific connect</Cmd>{" "}
                    detects your AI clients and writes their MCP config.{" "}
                    <Cmd>lific doctor</Cmd> tells you what&apos;s broken.
                  </>
                ),
              },
              {
                head: "Apache-2.0, no telemetry, no account.",
                body: (
                  <>
                    Permissive license, nothing phones home, and keys minted
                    from your own shell never require a signup.
                  </>
                ),
              },
            ]}
          />
        </section>

        {/* The literal 'use something else' section. */}
        <section className="band mt-[clamp(4.5rem,10vh,7rem)] py-[clamp(3rem,7vh,4.5rem)]">
          <H2 id="something-else">When to use something else</H2>
          <Body className="mb-8">
            These are honest defaults, not straw men. If one of these fits,
            use it — an issue tracker you resent is one you stop updating.
          </Body>

          <AltSection
            id="beads"
            title={
              <>
                Use <Ext href="https://github.com/steveyegge/beads">beads</Ext>{" "}
                if issues should live in the repo
              </>
            }
          >
            beads keeps a dependency-aware issue graph in a Dolt database
            inside your repository: it branches when you branch, merges
            without collisions, works offline, and needs no server process at
            all. If &quot;the tracker travels with the checkout&quot; is your
            requirement, beads is the purpose-built answer and Lific
            isn&apos;t. Lific&apos;s bet is the opposite one: a single tracker
            on your own server, spanning every project you have, shared by
            humans and agents through one URL.
          </AltSection>

          <AltSection
            id="gitea"
            title={
              <>
                Use <Ext href="https://about.gitea.com">Gitea</Ext> if issues
                should live next to the code forge
              </>
            }
          >
            If you already run Gitea — or want repos, pull requests, CI, and
            issues in one self-hosted instance — its first-party{" "}
            <Ext href="https://gitea.com/gitea/gitea-mcp">gitea-mcp</Ext>{" "}
            gives agents the whole forge, not just the tracker. The issue
            tools are simpler than a dedicated tracker&apos;s, but they come
            welded to the place your code already lives.
          </AltSection>

          <AltSection
            id="vikunja"
            title={
              <>
                Use <Ext href="https://vikunja.io">Vikunja</Ext> for personal
                task management
              </>
            }
          >
            Vikunja is a mature, polished to-do and project app — lists,
            kanban, Gantt, calendars — that happens to self-host beautifully.
            If you&apos;re organizing life and work rather than pointing
            coding agents at a backlog, it&apos;s the better tool. MCP access
            is community-maintained rather than first-party, so vet the server
            you pick.
          </AltSection>

          <AltSection
            id="plane"
            title={
              <>
                Use <Ext href="https://plane.so">Plane</Ext> for a full
                product-management suite
              </>
            }
          >
            Plane is an open-source Jira alternative: cycles, modules, docs,
            time tracking, triage, and a first-party MCP server you can use
            hosted or self-hosted. If you have a real team and need real PM
            machinery, Plane does far more than Lific — at the cost of a
            multi-service deployment instead of one binary.
          </AltSection>

          <AltSection
            id="linear"
            title={
              <>
                Use <Ext href="https://linear.app">Linear</Ext> if SaaS is fine
                and polish matters
              </>
            }
          >
            Linear is the best-run commercial tracker in the business, and its
            hosted MCP server is genuinely zero-setup: add a URL, OAuth in
            your browser, done. If you don&apos;t need self-hosting or your
            data on your own disk, it&apos;s the lowest-friction option on
            this page.
          </AltSection>
        </section>

        {/* Corrections */}
        <section className="mt-[clamp(3.5rem,8vh,5rem)]">
          <p className="max-w-[68ch] text-body leading-relaxed text-text-faint">
            This page is a snapshot: every claim above was checked against the
            linked sources on {STAMP}, and products ship faster than pages
            update. If a cell is wrong or has gone stale,{" "}
            <Ext href={ISSUES}>file an issue</Ext> — we&apos;ll fix the cell
            and the date.
          </p>
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

/* eslint-disable @next/next/no-img-element */
import type { Metadata } from "next";
import { SiteHeader } from "../components/SiteHeader";

const GITHUB = "https://github.com/VoidNullable/lific";
const CRATE = "https://crates.io/crates/lific";
const DISCORD = "https://discord.gg/uWvaFC4f7D";
const ISSUES = "https://github.com/VoidNullable/lific/issues";

// The snapshot date. Bump this whenever a cell is re-verified or
// corrected; the page is a dated snapshot, not an evergreen claim.
const STAMP = "July 14, 2026";
const STAMP_ISO = "2026-07-14";

export const metadata: Metadata = {
  title: "Issue trackers with MCP support, compared · Lific",
  description:
    "A date-stamped comparison of issue trackers with MCP servers: Lific, beads, Vikunja, Gitea, Plane, and Linear. First-party support, transports, measured tool counts and token costs, deployment, licenses, and honest losses.",
  alternates: { canonical: "/compare" },
  openGraph: {
    title: "Issue trackers with MCP support, compared",
    description:
      "Lific vs beads, Vikunja, Gitea, Plane, and Linear: real tables, measured token costs, real losses, and a literal 'when to use something else' section.",
    url: "https://lific.dev/compare",
    siteName: "Lific",
    type: "article",
    publishedTime: STAMP_ISO,
    modifiedTime: STAMP_ISO,
    images: [
      {
        url: "/og.png",
        width: 1920,
        height: 1080,
        alt: "The Lific logo over the kanban board of the Lific web UI",
      },
    ],
  },
  twitter: {
    card: "summary_large_image",
    title: "Issue trackers with MCP support, compared",
    description:
      "Lific vs beads, Vikunja, Gitea, Plane, and Linear: real tables, measured token costs, real losses.",
    images: ["/og.png"],
  },
};

// Structured data: a dated TechArticle (snapshot comparison with a real
// publication date), breadcrumbs, and an FAQPage whose entries mirror the
// visible FAQ section verbatim.
const FAQS: { q: string; a: string }[] = [
  {
    q: "How is Lific different from beads?",
    a: "beads stores a dependency-aware issue graph inside your repository and needs no server: the tracker branches, merges, and travels with the checkout. Lific is one server outside any single repository, tracking every project you have, with a web UI for humans and MCP, REST, and CLI for agents. If the tracker should live in the repo, use beads. If humans and agents should share one tracker across all projects, use Lific.",
  },
  {
    q: "Which issue trackers have first-party MCP servers?",
    a: "As of July 14, 2026: Lific (built into the tracker binary), beads (beads-mcp), Gitea (gitea-mcp), Plane (plane-mcp-server, also hosted at mcp.plane.so), and Linear (hosted at mcp.linear.app). Vikunja has no first-party server; several community MCP servers exist.",
  },
  {
    q: "How many context tokens does an issue tracker's MCP server cost?",
    a: "Measured against each server's tools/list output with tiktoken o200k_base on July 14, 2026: beads 2,871 tokens (15 tools), Lific 5,641 (27 tools), Gitea 6,676 (53 tools), a Vikunja community server 7,213 (53 tools), and Plane 30,105 (139 tools). MCP client guidance recommends budgeting tool definitions to roughly 1 to 5 percent of the context window, which is 2k to 10k tokens on a 200k model.",
  },
];

const JSONLD = JSON.stringify([
  {
    "@context": "https://schema.org",
    "@type": "TechArticle",
    headline: "Issue trackers with MCP support, compared",
    description:
      "A comparison of issue trackers usable by coding agents over the Model Context Protocol: Lific, beads, Vikunja, Gitea, Plane, and Linear.",
    datePublished: STAMP_ISO,
    dateModified: STAMP_ISO,
    url: "https://lific.dev/compare",
    author: {
      "@type": "Organization",
      name: "Lific",
      url: "https://lific.dev",
    },
  },
  {
    "@context": "https://schema.org",
    "@type": "BreadcrumbList",
    itemListElement: [
      {
        "@type": "ListItem",
        position: 1,
        name: "Lific",
        item: "https://lific.dev/",
      },
      {
        "@type": "ListItem",
        position: 2,
        name: "Compare",
        item: "https://lific.dev/compare",
      },
    ],
  },
  {
    "@context": "https://schema.org",
    "@type": "FAQPage",
    mainEntity: FAQS.map(({ q, a }) => ({
      "@type": "Question",
      name: q,
      acceptedAnswer: { "@type": "Answer", text: a },
    })),
  },
]);

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
    <div className="mt-8 w-full min-w-0 max-w-full overflow-x-auto rounded-xl border border-border bg-bg-subtle/40">
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

/*
 * The at-a-glance boolean matrix. Same rules as the prose tables: real
 * <table> markup, real scoped headers. Cells are strictly yes/no; the
 * footnotes carry the only nuance. Marks get aria-labels so screen
 * readers hear "Yes"/"No" instead of glyph names.
 */
function Mark({ yes, note }: { yes: boolean; note?: number }) {
  return (
    <span className="whitespace-nowrap">
      <span
        role="img"
        aria-label={yes ? "Yes" : "No"}
        className={
          yes ? "font-semibold text-success" : "text-text-faint/70"
        }
      >
        {yes ? "\u2713" : "\u2717"}
      </span>
      {note !== undefined && (
        <sup>
          <a
            href={`#glance-note-${note}`}
            aria-label={`Footnote ${note}`}
            className="ml-0.5 text-micro text-text-faint hover:text-accent"
          >
            {note}
          </a>
        </sup>
      )}
    </span>
  );
}

const TRACKERS = ["Lific", "beads", "Vikunja", "Gitea", "Plane", "Linear"];

// Each row: [feature label, then one cell per tracker in TRACKERS order].
// A cell is boolean, or [boolean, footnoteNumber].
type GlanceCell = boolean | [boolean, number];
const GLANCE_ROWS: [string, ...GlanceCell[]][] = [
  ["First-party MCP server", true, true, [false, 1], true, true, true],
  ["Tracker and MCP server are one process", true, false, false, false, false, false],
  ["Built for coding agents first", true, true, false, false, false, false],
  ["Ready-work query (unblocked issues, one call)", true, true, false, false, false, false],
  ["Web UI for humans included", true, false, true, true, true, true],
  ["Self-host with a single binary", true, [true, 2], true, true, false, false],
  ["Free and open source", true, true, true, true, true, false],
  ["Repo-local mode (tracker lives in your repo)", false, true, false, false, false, false],
  ["Hosted cloud option", false, false, true, true, true, true],
];

const GLANCE_NOTES = [
  "Community-maintained MCP servers for Vikunja exist; none are first-party.",
  "beads goes further: it has no server at all, just a CLI with an embedded database.",
];

function GlanceTable() {
  return (
    <>
      <div className="mt-8 w-full min-w-0 max-w-full overflow-x-auto rounded-xl border border-border bg-bg-subtle/40">
        <table className="w-full min-w-[720px] border-collapse">
          <caption className="sr-only">
            Feature support across issue trackers with MCP: Lific, beads,
            Vikunja, Gitea, Plane, and Linear
          </caption>
          <thead>
            <tr>
              <th scope="col" className={`${th} w-[38%]`}>
                <span className="sr-only">Feature</span>
              </th>
              {TRACKERS.map((t) => (
                <th
                  scope="col"
                  key={t}
                  className={`${th} text-center ${
                    t === "Lific" ? "bg-accent-subtle/40" : ""
                  }`}
                >
                  {t}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {GLANCE_ROWS.map(([feature, ...cells]) => (
              <tr key={feature}>
                <th
                  scope="row"
                  className="border-b border-border/60 px-4 py-3 text-left align-middle text-body-sm font-medium leading-snug text-text"
                >
                  {feature}
                </th>
                {cells.map((cell, i) => {
                  const [yes, note] = Array.isArray(cell)
                    ? cell
                    : ([cell, undefined] as const);
                  return (
                    <td
                      key={TRACKERS[i]}
                      className={`border-b border-border/60 px-4 py-3 text-center align-middle text-body ${
                        i === 0 ? "bg-accent-subtle/40" : ""
                      }`}
                    >
                      <Mark yes={yes} note={note} />
                    </td>
                  );
                })}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <ol className="mt-3 max-w-[75ch] text-caption leading-relaxed text-text-faint">
        {GLANCE_NOTES.map((note, i) => (
          <li key={i} id={`glance-note-${i + 1}`} className="scroll-mt-24">
            {i + 1}. {note}
          </li>
        ))}
      </ol>
    </>
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
// evidence. Used for both the losses and the wins, same shape; the
// symmetry is the point.
function FactList({
  items,
}: {
  items: { head: React.ReactNode; body: React.ReactNode }[];
}) {
  return (
    <ul className="mt-8 min-w-0 max-w-4xl">
      {items.map(({ head, body }, i) => (
        <li
          key={i}
          className="min-w-0 border-t border-border/60 py-4 text-body leading-relaxed last:border-b"
        >
          <p className="min-w-0 [overflow-wrap:anywhere] font-medium text-text">
            {head}
          </p>
          <p className="mt-0.5 min-w-0 max-w-[75ch] [overflow-wrap:anywhere] text-text-faint [&_code]:max-w-full [&_code]:whitespace-normal [&_code]:[overflow-wrap:anywhere] sm:[&_code]:whitespace-nowrap">
            {body}
          </p>
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
    <section
      id={id}
      className="min-w-0 scroll-mt-24 border-t border-border/60 py-6"
    >
      <h3 className="min-w-0 font-display text-title font-semibold tracking-tight [overflow-wrap:anywhere] sm:[overflow-wrap:normal]">
        {title}
      </h3>
      <p className="mt-2 min-w-0 max-w-[75ch] text-body leading-relaxed text-text-muted [overflow-wrap:anywhere] sm:[overflow-wrap:normal]">
        {children}
      </p>
    </section>
  );
}

export default function Compare() {
  return (
    <div className="min-w-0 flex-1">
      <script
        type="application/ld+json"
        dangerouslySetInnerHTML={{ __html: JSONLD }}
      />

      <SiteHeader page="compare" />

      <main className="mx-auto w-full min-w-0 max-w-5xl px-6 pb-24">
        {/* Title */}
        <section className="pt-[clamp(3.5rem,10vh,6rem)]">
          <p className="text-micro font-semibold uppercase tracking-widest text-text-faint">
            Comparison
          </p>
          <h1 className="mt-5 max-w-[22ch] break-words font-display text-[clamp(2rem,5.5vw,3.75rem)] font-semibold leading-[1.08] tracking-tight">
            Issue trackers with MCP support,{" "}
            <span className="brand-gradient-text">compared.</span>
          </h1>
          {/* Article-style byline. This is the disclosure: the reader sees
              who wrote the comparison before any table cell. */}
          <p className="mt-6 flex flex-col items-start gap-1 text-body-lg text-text-muted sm:flex-row sm:flex-wrap sm:items-center sm:gap-x-3 sm:gap-y-1">
            <span>
              Written by{" "}
              <a
                href="/"
                className="font-medium text-text underline decoration-border underline-offset-4 transition-colors hover:text-accent hover:decoration-accent"
              >
                Lific.dev
              </a>
            </span>
            <span aria-hidden className="hidden text-text-faint sm:inline">
              ·
            </span>
            <span>Accurate at time of writing, {STAMP}</span>
          </p>
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
            Every cell below is checkable against the linked primary sources,
            the products we compare against are good at what they do, and
            Lific&apos;s flaws get a section of their own. No cell pretends
            they don&apos;t exist.
          </Body>
        </section>


        {/* At a glance: the boolean matrix. This is the row-level truth of
            the prose tables below, compressed to yes/no so the shape of the
            comparison is visible before any reading happens. Keep cells
            strictly boolean; nuance lives in the footnotes and the sections
            underneath. */}
        <section className="mt-[clamp(4rem,9vh,6rem)]">
          <H2 id="at-a-glance">At a glance</H2>
          <Body>
            The details, transports, and sources for every row are in the
            sections below.
          </Body>
          <GlanceTable />
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
                    27 tools: issues, nestable plans, pages, comments, search,
                    audit history. The whole surface costs about 5.6k tokens
                    of context (measured below).
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
                    15 tools: dependency-aware issue graph, ready-work
                    detection, persistent agent memory. beads&apos; own docs
                    recommend the CLI over MCP when the agent has a shell,
                    since it costs fewer tokens.
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
                    Tasks, projects, labels, and kanban, through whichever
                    community server you pick and vet yourself. The one we
                    measured exposes 53 tools; another documents 180.
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
                    53 forge-level tools. Issues get CRUD, comments, and
                    edits; most of the surface is repositories, files, and
                    pull requests.
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
                    Plane&apos;s full API surface as 139 tools: work items,
                    sprints, modules, docs, time tracking.
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
                    . Nothing to install, and nothing to self-host.
                  </>,
                  <>Streamable HTTP (OAuth 2.1)</>,
                  <>
                    A curated set of about two dozen tools over Linear&apos;s
                    API: issues, projects, comments, documents. Requires a
                    Linear account.
                  </>,
                ],
              },
            ]}
          />
        </section>

        {/* The context bill: measured tool counts and schema token costs.
            These are our own measurements (methodology below the table),
            anchored to the MCP project's published budget guidance so the
            numbers have a yardstick, not just vibes. */}
        <section className="mt-[clamp(4.5rem,10vh,7rem)]">
          <H2 id="context-bill">The context bill: tool counts and token costs</H2>
          <Body>
            Tool definitions are not free. Every MCP server an agent connects
            to injects its full tool schemas into the context window before
            any work happens. The MCP project&apos;s own{" "}
            <Ext href="https://modelcontextprotocol.io/docs/develop/clients/client-best-practices">
              client guidance
            </Ext>{" "}
            is blunt about it: loading every tool definition up front
            &quot;wastes tokens, increases latency, and degrades model
            performance&quot;, and it suggests budgeting tool definitions to
            roughly 1 to 5 percent of the context window. On a 200k-token
            model, that is 2k to 10k tokens.{" "}
            <Ext href="https://www.anthropic.com/engineering/writing-tools-for-agents">
              Anthropic&apos;s guidance
            </Ext>{" "}
            for tool authors is shorter: &quot;More tools don&apos;t always
            lead to better outcomes.&quot;
          </Body>
          <Body>
            So we measured, on {STAMP}: each server launched over stdio,
            asked for its tool list, schemas tokenized. Numbers below are what
            an agent pays before it reads a single line of your code.
          </Body>
          <ComparisonTable
            caption="Measured tool counts and tool-schema token costs of each tracker's MCP server"
            head={[
              "Server measured",
              "Tools",
              "Schema size",
              "Share of a 200k context",
            ]}
            rows={[
              {
                name: "Lific",
                lific: true,
                cells: [
                  <>
                    Built in: <Cmd>lific mcp</Cmd> (v2.2.1)
                  </>,
                  <>27</>,
                  <>5,641 tokens</>,
                  <>2.8%</>,
                ],
              },
              {
                name: "beads",
                cells: [
                  <>
                    <Ext href="https://pypi.org/project/beads-mcp/">
                      beads-mcp
                    </Ext>{" "}
                    (PyPI)
                  </>,
                  <>15</>,
                  <>2,871 tokens</>,
                  <>1.4%</>,
                ],
              },
              {
                name: "Vikunja",
                cells: [
                  <>
                    <Ext href="https://github.com/aimbitgmbh/vikunja-mcp">
                      @aimbitgmbh/vikunja-mcp
                    </Ext>{" "}
                    (community)
                  </>,
                  <>53</>,
                  <>7,213 tokens</>,
                  <>3.6%</>,
                ],
              },
              {
                name: "Gitea",
                cells: [
                  <>
                    <Ext href="https://gitea.com/gitea/gitea-mcp">
                      gitea-mcp
                    </Ext>{" "}
                    v1.3.0
                  </>,
                  <>53</>,
                  <>6,676 tokens</>,
                  <>3.3%</>,
                ],
              },
              {
                name: "Plane",
                cells: [
                  <>
                    <Ext href="https://pypi.org/project/plane-mcp-server/">
                      plane-mcp-server
                    </Ext>{" "}
                    (PyPI)
                  </>,
                  <>139</>,
                  <>30,105 tokens</>,
                  <>15.1%</>,
                ],
              },
              {
                name: "Linear",
                cells: [
                  <>Hosted at mcp.linear.app</>,
                  <>
                    23{" "}
                    <Ext href="https://blog.fiberplane.com/blog/mcp-server-analysis-linear/">
                      (published)
                    </Ext>
                  </>,
                  <>Not measurable without a workspace login</>,
                  <>unknown</>,
                ],
              },
            ]}
          />
          <Body>
            Four of the five measurable servers fit the budget. beads deserves
            specific credit for the smallest bill: its server hides most
            commands behind a <Cmd>discover_tools</Cmd> call instead of
            declaring everything up front. (That same design means its
            measured number understates the eventual cost, since full schemas
            load on demand.) Plane is the outlier. 139 tools and thirty
            thousand tokens is 15 percent of a 200k context spent before any
            work starts, and it exceeds{" "}
            <Ext href="https://code.visualstudio.com/docs/chat/chat-tools">
              VS Code&apos;s 128-tools-per-request cap
            </Ext>{" "}
            on its own. The count is not padding, it is philosophy: in our
            dump, five of every six Plane tools are create, read, update, or
            delete variants spread across about two dozen entity types, from
            work items down to estimate points and property options. That is
            an API mirror, and agents pay for the whole mirror up front.
            Linear&apos;s hosted server sits behind a workspace login, so we
            cite the published count instead of measuring it ourselves;
            Linear also revises its tool set regularly.
          </Body>
          <Body>
            Lific&apos;s own 5.6k fits the budget, but not gracefully: per
            tool, its schemas are the second wordiest on this page, after
            Plane.
          </Body>
          <p className="mt-4 max-w-[75ch] text-caption leading-relaxed text-text-faint">
            Methodology: each server was launched over stdio on {STAMP},
            sent <Cmd>initialize</Cmd> and <Cmd>tools/list</Cmd> via the
            official MCP Python SDK, and the returned tool definitions (name,
            description, input schema) were serialized as compact JSON and
            tokenized with tiktoken&apos;s o200k_base. Clients serialize
            schemas differently, so treat small deltas as noise; the order of
            magnitude is the point.
          </p>
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
                    binaries for Linux and macOS. Windows is cargo-only, with
                    no prebuilt binary.
                  </>,
                  <>SQLite</>,
                  <>None; self-host only</>,
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
                  <>None; local by design</>,
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
                    Single Go binary, or Docker. A whole code forge, not just
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
                  <>Nothing to run; it&apos;s SaaS.</>,
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
                    About 5.6k tokens for the full 27-tool surface, roughly
                    one long file read, so connecting the tracker doesn&apos;t
                    crowd out the actual work.
                  </>
                ),
              },
              {
                head: "Humans get a real UI in the same binary.",
                body: (
                  <>
                    Issue list, kanban board, documents, and comment threads
                    where you and your agents talk to each other, at{" "}
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

        {/* The flaws, stated plainly, after the case is made. These facts
            get surfaced whether or not we state them; stating them ourselves
            means the framing is ours. */}
        <section className="mt-[clamp(4.5rem,10vh,7rem)]">
          <H2 id="losses">Where Lific loses</H2>
          <Body>
            Everything above is true, and so is this. If one of these is a
            dealbreaker for you, the next section points you somewhere good.
          </Body>
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
                    <Cmd>cargo install lific</Cmd>. There is also no Windows
                    service integration, so keeping the server running is on
                    you (Task Scheduler or a terminal).
                  </>
                ),
              },
              {
                head: "No repo-local mode.",
                body: (
                  <>
                    Issues live in a server database, not in your repository.
                    If you want the tracker versioned with the code (branching
                    with it, merging with it, readable offline in the
                    checkout), <a href="#beads" className="text-text underline decoration-border underline-offset-4 hover:text-accent hover:decoration-accent">beads does exactly that</a>{" "}
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

        {/* The literal 'use something else' section. */}
        <section className="band min-w-0 mt-[clamp(4.5rem,10vh,7rem)] py-[clamp(3rem,7vh,4.5rem)]">
          <H2 id="something-else">When to use something else</H2>
          <Body className="mb-8">
            These are honest defaults, not straw men. If one of these fits,
            use it. An issue tracker you resent is one you stop updating.
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
            If you already run Gitea, or want repos, pull requests, CI, and
            issues in one self-hosted instance, its first-party{" "}
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
            Vikunja is a mature, polished to-do and project app (lists,
            kanban, Gantt, calendars) that happens to self-host beautifully.
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
            machinery, Plane does far more than Lific, at the cost of a
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

        {/* FAQ: rendered from the same FAQS array as the FAQPage JSON-LD,
            so the structured data can never drift from the visible text. */}
        <section className="mt-[clamp(4.5rem,10vh,7rem)] min-w-0">
          <H2 id="faq">Questions people actually ask</H2>
          <div className="mt-8 min-w-0 max-w-4xl">
            {FAQS.map(({ q, a }) => (
              <section
                key={q}
                className="min-w-0 border-t border-border/60 py-5 last:border-b"
              >
                <h3 className="min-w-0 break-words font-display text-title font-semibold tracking-tight md:break-normal">
                  {q}
                </h3>
                <p className="mt-2 min-w-0 max-w-[75ch] break-words text-body leading-relaxed text-text-muted md:break-normal">
                  {a}
                </p>
              </section>
            ))}
          </div>
        </section>

        {/* Corrections */}
        <section className="mt-[clamp(3.5rem,8vh,5rem)] min-w-0">
          <p className="min-w-0 max-w-[68ch] break-words text-body leading-relaxed text-text-faint md:break-normal">
            This page is a snapshot: every claim above was checked against the
            linked sources on {STAMP}, and products ship faster than pages
            update. If a cell is wrong or has gone stale,{" "}
            <Ext href={ISSUES}>file an issue</Ext>
            {" and we'll fix the cell and the date."}
          </p>
        </section>
      </main>

      <footer>
        <div className="mx-auto flex w-full max-w-5xl flex-col items-start gap-5 px-6 py-8 font-mono text-caption text-text-faint sm:flex-row sm:flex-wrap sm:items-center sm:justify-between sm:gap-4">
          <span className="flex w-full min-w-0 items-center gap-2 sm:w-auto">
            <img
              src="/logo.webp"
              alt=""
              width={16}
              height={16}
              className="shrink-0 rounded"
            />
            © 2026{"\u00a0·\u00a0"}Apache-2.0{"\u00a0·\u00a0"}no telemetry
          </span>
          <div className="flex w-full flex-wrap items-center gap-x-5 gap-y-1 sm:w-auto sm:gap-y-0">
            <a className="-mx-1 px-1 py-3 transition-colors hover:text-text sm:mx-0 sm:px-0 sm:py-0" href={GITHUB}>
              github
            </a>
            <a className="-mx-1 px-1 py-3 transition-colors hover:text-text sm:mx-0 sm:px-0 sm:py-0" href={CRATE}>
              crates.io
            </a>
            <a className="-mx-1 px-1 py-3 transition-colors hover:text-text sm:mx-0 sm:px-0 sm:py-0" href={DISCORD}>
              discord
            </a>
          </div>
        </div>
      </footer>
    </div>
  );
}

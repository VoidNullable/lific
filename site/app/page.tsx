/* eslint-disable @next/next/no-img-element */
import type { Metadata } from "next";
import { CopyButton } from "./components/CopyButton";
import { SiteHeader } from "./components/SiteHeader";
import { ForAgents } from "./components/sections/ForAgents";
import { ForHumans } from "./components/sections/ForHumans";
import { ForTeams } from "./components/sections/ForTeams";
import { ForEveryone } from "./components/sections/ForEveryone";

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
    // Prebuilt release binaries cover Linux and macOS (x86_64 + arm64) and
    // Windows x86_64; `cargo install` works everywhere Rust does.
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




export default function Home() {
  return (
    <div className="flex-1">
      <script
        type="application/ld+json"
        dangerouslySetInnerHTML={{ __html: JSONLD }}
      />
      <SiteHeader page="home" />

      <main className="mx-auto w-full max-w-5xl px-6">
        {/* Hero */}
        <section className="pt-[clamp(4rem,12vh,7rem)] text-center">
          <div className="animate-reveal flex justify-center">
            <SectionLabel>
              Native MCP · one binary · self-hosted · free &amp; open source
            </SectionLabel>
          </div>
          <h1 className="animate-reveal delay-100 mx-auto mt-5 font-display text-[clamp(2.375rem,7vw,4.75rem)] font-semibold leading-[1.08] tracking-tight md:text-[clamp(2.75rem,7vw,4.75rem)]">
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
            <div className="flex flex-wrap items-center justify-between gap-4 rounded-lg border border-border bg-surface py-4 pl-6 pr-3 text-left shadow-lg">
              <code className="min-w-0 max-w-full flex-1 overflow-x-auto whitespace-pre font-mono text-[clamp(0.95rem,2vw,1.15rem)] leading-relaxed">
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
              (Linux and macOS, x86_64 and arm64, and Windows x86_64)
            </p>
          </div>
        </section>

        <ForAgents />

        <ForHumans />

        <ForTeams />

        <ForEveryone />

        {/* Closing */}
        <section className="band band-finale">
          <div className="flex flex-col items-start gap-8 py-[clamp(3rem,7vh,4.5rem)] sm:flex-row sm:items-center sm:justify-between">
            <div>
              <p className="max-w-[28ch] font-display text-[clamp(1.75rem,4vw,2.75rem)] font-semibold leading-tight tracking-tight">
                Issue trackers should be simple,{" "}
                <span className="brand-gradient-text">right?</span>
              </p>
              <div className="mt-8 flex w-full flex-col items-stretch gap-4 sm:w-auto sm:flex-row sm:flex-wrap sm:items-center">
                <a
                  href={GITHUB}
                  className="w-full rounded-md bg-btn-success px-4 py-2.5 text-center text-body-lg font-medium text-btn-success-text transition-colors hover:bg-btn-success-hover motion-safe:active:scale-[0.97] motion-safe:transition-transform sm:w-auto"
                >
                  Star on GitHub
                </a>
                <code className="w-full max-w-full overflow-x-auto whitespace-nowrap rounded-md border border-border px-3 py-2 font-mono text-body-sm text-text-muted sm:w-auto sm:max-w-none sm:overflow-visible">
                  <span className="select-none text-success">$ </span>cargo
                  install lific
                </code>
              </div>
            </div>
          </div>
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
            <a className="-mx-1 px-1 py-3 transition-colors hover:text-text sm:mx-0 sm:px-0 sm:py-0" href="/compare">
              compare
            </a>
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

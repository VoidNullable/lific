import { StarCount } from "./StarCount";
import { VersionChip } from "./VersionChip";

const GITHUB = "https://github.com/VoidNullable/lific";
const CRATE = "https://crates.io/crates/lific";
const DISCORD = "https://discord.gg/uWvaFC4f7D";

export function SiteHeader({ page }: { page: "home" | "compare" }) {
  const isCompare = page === "compare";

  return (
    /* Sticky chrome bar, borrowed from the product's brand header
        (Layout.svelte): logo + font-display wordmark + mono version
        chip on --chrome. */
    <header className="sticky top-3 z-30 mx-auto w-full max-w-5xl px-2.5 sm:px-6">
      <div className="flex items-center gap-2 rounded-xl border border-border bg-chrome px-2.5 py-2 shadow-lg sm:gap-2.5 sm:px-3">
        <a
          href={isCompare ? "/" : GITHUB}
          className="group flex min-w-0 items-center gap-1.5 rounded-lg px-1 py-1 transition-colors hover:bg-bg-subtle focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent sm:gap-2.5"
          title={isCompare ? "Lific home" : "View Lific on GitHub"}
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
            className="flex h-7 items-center rounded-md px-1.5 text-caption font-medium text-text-muted transition-colors hover:bg-bg-subtle hover:text-text focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent sm:px-2"
            href="/docs"
          >
            Docs
          </a>
          <a
            className={
              isCompare
                ? "flex h-7 items-center rounded-md bg-bg-subtle px-1.5 text-caption font-medium text-text focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent sm:px-2"
                : "flex h-7 items-center rounded-md px-1.5 text-caption font-medium text-text-muted transition-colors hover:bg-bg-subtle hover:text-text focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent sm:px-2"
            }
            href="/compare"
            aria-current={isCompare ? "page" : undefined}
          >
            Compare
          </a>
          {page === "home" && (
            <a
              className="hidden h-7 items-center rounded-md px-2 text-caption font-medium text-text-muted transition-colors hover:bg-bg-subtle hover:text-text sm:flex focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent"
              href={CRATE}
            >
              crates.io
            </a>
          )}
          <a
            className="hidden h-7 items-center rounded-md px-2 text-caption font-medium text-text-muted transition-colors hover:bg-bg-subtle hover:text-text sm:flex focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent"
            href={DISCORD}
          >
            Discord
          </a>
          <a
            className="flex h-7 items-center rounded-md px-1.5 text-caption font-medium text-text-muted transition-colors hover:bg-bg-subtle hover:text-text focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent sm:px-2"
            href={GITHUB}
          >
            GitHub
            <span className="hidden sm:flex">
              <StarCount />
            </span>
          </a>
          {/* Green primary CTA. Indigo is the accent, green is the
              action color (LIF-DOC-14 §3). */}
          <a
            className="ml-1 rounded-md bg-btn-success px-2.5 py-1.5 text-body-sm font-medium text-btn-success-text transition-colors hover:bg-btn-success-hover motion-safe:active:scale-[0.97] motion-safe:transition-transform focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent sm:px-3"
            href={isCompare ? "/#install" : "#install"}
          >
            Install
          </a>
        </nav>
      </div>
    </header>
  );
}

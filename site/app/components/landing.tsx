// Shared building blocks for the landing page sections.

// The four-beat pitch title: "For <audience>" IS the heading.
export function SectionTitle({ children }: { children: React.ReactNode }) {
  return (
    <h2 className="font-display text-[clamp(2.25rem,5vw,3.5rem)] font-semibold leading-none tracking-tight">
      <span className="text-text-faint">For</span> {children}
    </h2>
  );
}

// Landing-prose paragraph: bigger than app body, muted, capped measure.
export function Body({
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
export function Em({ children }: { children: React.ReactNode }) {
  return <span className="font-medium text-text">{children}</span>;
}

// Inline command chip, the product's .prose code recipe (app.css):
// mono, bordered, on the focal surface tier so it reads on both the
// page floor and the section bands.
export function Cmd({ children }: { children: React.ReactNode }) {
  return (
    <code className="whitespace-nowrap rounded-[4px] border border-border bg-surface px-[0.4em] py-[0.15em] font-mono text-[0.8125em] text-text">
      {children}
    </code>
  );
}
// Shared window chrome for every media artifact on the page: videos
// and terminals live in the same shell, the way they would on a desk.
export function Window({
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


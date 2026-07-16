"use client";

import { useEffect, useState } from "react";

// Module-level cache: dedupes Strict Mode double-effects and repeat
// mounts so a session makes at most one GitHub request.
let cachedStars: number | null = null;
let inflight: Promise<number | null> | null = null;

function fetchStars(): Promise<number | null> {
  if (cachedStars !== null) return Promise.resolve(cachedStars);
  inflight ??= fetch("https://api.github.com/repos/VoidNullable/lific", {
    headers: { Accept: "application/vnd.github+json" },
  })
    .then((r) => (r.ok ? r.json() : null))
    .then((d) => {
      if (d && typeof d.stargazers_count === "number") {
        cachedStars = d.stargazers_count;
        return cachedStars;
      }
      return null;
    })
    .catch(() => null);
  return inflight;
}

// Stars only. Deliberately nothing else from the repo API. The slot is
// pre-reserved so the sticky header never shifts when the count lands.
export function StarCount() {
  const [stars, setStars] = useState<number | null>(cachedStars);

  useEffect(() => {
    let alive = true;
    fetchStars().then((n) => {
      if (alive && n !== null) setStars(n);
    });
    return () => {
      alive = false;
    };
  }, []);

  const label =
    stars === null
      ? null
      : stars >= 1000
        ? `${(stars / 1000).toFixed(1).replace(/\.0$/, "")}k`
        : String(stars);

  return (
    <span
      aria-hidden={label === null}
      className={`ml-1.5 inline-flex min-w-10 items-center justify-center gap-1 rounded-md bg-bg-subtle px-1.5 py-0.5 font-mono text-micro text-text-faint transition-opacity duration-300 ${
        label === null ? "opacity-0" : "opacity-100"
      }`}
    >
      <svg
        viewBox="0 0 24 24"
        width={10}
        height={10}
        fill="none"
        stroke="currentColor"
        strokeWidth={2}
        strokeLinecap="round"
        strokeLinejoin="round"
        aria-hidden="true"
        className="shrink-0"
      >
        <path d="M11.525 2.295a.53.53 0 0 1 .95 0l2.31 4.679a2.123 2.123 0 0 0 1.595 1.16l5.166.756a.53.53 0 0 1 .294.904l-3.736 3.638a2.123 2.123 0 0 0-.611 1.878l.882 5.14a.53.53 0 0 1-.771.56l-4.618-2.428a2.122 2.122 0 0 0-1.973 0L6.396 21.01a.53.53 0 0 1-.77-.56l.881-5.139a2.122 2.122 0 0 0-.611-1.879L2.16 9.795a.53.53 0 0 1 .294-.906l5.165-.755a2.122 2.122 0 0 0 1.597-1.16z" />
      </svg>
      {label}
    </span>
  );
}

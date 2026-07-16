"use client";

import { useEffect, useState } from "react";

// Module-level cache, same discipline as StarCount: one request per
// session, Strict Mode double-effects deduped.
let cachedVersion: string | null = null;
let inflight: Promise<string | null> | null = null;

function fetchLatestVersion(): Promise<string | null> {
  if (cachedVersion !== null) return Promise.resolve(cachedVersion);
  inflight ??= fetch(
    "https://api.github.com/repos/VoidNullable/lific/releases/latest",
    { headers: { Accept: "application/vnd.github+json" } },
  )
    .then((r) => (r.ok ? r.json() : null))
    .then((d) => {
      if (d && typeof d.tag_name === "string") {
        cachedVersion = d.tag_name.replace(/^v/, "");
        return cachedVersion;
      }
      return null;
    })
    .catch(() => null);
  return inflight;
}

/**
 * The brand header's version chip. The slot is pre-reserved so the
 * header never shifts; the chip fades in only once the latest GitHub
 * release tag actually arrives. Offline or rate-limited, it simply
 * never appears.
 */
export function VersionChip() {
  const [version, setVersion] = useState<string | null>(cachedVersion);

  useEffect(() => {
    let alive = true;
    fetchLatestVersion().then((v) => {
      if (alive && v !== null) setVersion(v);
    });
    return () => {
      alive = false;
    };
  }, []);

  return (
    <span
      aria-hidden={version === null}
      className={`hidden min-w-12 rounded-md bg-bg-subtle px-1.5 py-0.5 text-center font-mono text-micro tracking-tight text-text-faint transition-opacity duration-300 group-hover:bg-surface sm:inline-block ${
        version === null ? "opacity-0" : "opacity-100"
      }`}
    >
      {version === null ? null : `v${version}`}
    </span>
  );
}

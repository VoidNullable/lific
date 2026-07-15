import { DocsLayout } from "fumadocs-ui/layouts/docs";
import { RootProvider } from "fumadocs-ui/provider/next";
import { source } from "@/lib/source";
import type { ReactNode } from "react";

const GITHUB = "https://github.com/VoidNullable/lific";

export default function Layout({ children }: { children: ReactNode }) {
  return (
    <RootProvider
      // The site is dark-only; the `dark` class is set on <html> in the
      // root layout, so next-themes is unnecessary.
      theme={{ enabled: false }}
      // Static export: search queries the build-time index at /api/search.
      search={{ options: { type: "static" } }}
    >
      <DocsLayout
        tree={source.pageTree}
        githubUrl={GITHUB}
        nav={{
          title: (
            <span className="flex items-center gap-2">
              {/* eslint-disable-next-line @next/next/no-img-element */}
              <img
                src="/logo.webp"
                alt=""
                width={22}
                height={22}
                className="shrink-0 rounded-md"
              />
              <span className="font-display text-heading leading-none tracking-tight">
                Lific
              </span>
            </span>
          ),
          url: "/",
        }}
        themeSwitch={{ enabled: false }}
      >
        {children}
      </DocsLayout>
    </RootProvider>
  );
}

import type { Metadata, Viewport } from "next";
import { DM_Sans, Space_Grotesk } from "next/font/google";
import "./globals.css";

// Same faces the product web UI loads (web/index.html).
const spaceGrotesk = Space_Grotesk({
  variable: "--font-space-grotesk",
  subsets: ["latin"],
  weight: ["400", "500", "600", "700"],
});

const dmSans = DM_Sans({
  variable: "--font-dm-sans",
  subsets: ["latin"],
});

export const metadata: Metadata = {
  metadataBase: new URL("https://lific.dev"),
  title: "Lific · An issue tracker for prolific agents",
  description:
    "A free, self-hosted issue tracker built for coding agents. Single binary, native MCP. Plans and issues live on your server instead of in the context window, so work outlives the session.",
  alternates: { canonical: "https://lific.dev" },
  openGraph: {
    title: "Lific · An issue tracker for prolific agents",
    description:
      "An issue tracker built for coding agents. Plans and issues live on your server instead of in the context window, so work outlives the session.",
    url: "https://lific.dev",
    siteName: "Lific",
    type: "website",
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
    title: "Lific · An issue tracker for prolific agents",
    description:
      "An issue tracker built for coding agents. Plans and issues live on your server instead of in the context window.",
    images: ["/og.png"],
  },
};

export const viewport: Viewport = {
  themeColor: "#0d1110",
};

export default function RootLayout({
  children,
}: Readonly<{
  children: React.ReactNode;
}>) {
  return (
    <html
      lang="en"
      // `dark` is permanent: the site has one (dark) theme, and the
      // fumadocs styles on /docs key their dark palette off this class.
      className={`${spaceGrotesk.variable} ${dmSans.variable} dark h-full antialiased`}
    >
      <body className="min-h-full flex flex-col">
        {/* Reveal gates content behind JS; without it, show everything. */}
        <noscript>
          <style>{`.reveal-pending{opacity:1}`}</style>
        </noscript>
        {children}
      </body>
    </html>
  );
}

# lific.dev

Marketing/landing page for Lific, hosted at https://lific.dev.

Next.js 16 (App Router) + Tailwind v4, managed with Bun. Fully static — no
server-side anything.

```bash
bun install
bun run dev     # local dev on :3000
bun run build   # production build
```

Content facts (the three numbers, install commands) mirror the root
`README.md`; keep them in sync when the main README changes.

`public/board-loop.mp4` is rendered from the Remotion project in `../promo`:

```bash
cd ../promo && bunx remotion render BoardLoop ../site/public/board-loop.mp4
```

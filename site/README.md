# lific.dev

Marketing/landing page for Lific, hosted at https://lific.dev.

Next.js 16 (App Router) + Tailwind v4, managed with Bun. Fully static — no
server-side anything.

Use the repository's `docs` profile for the site:

```bash
devenv --profile docs tasks run lific:docs:check
```

Content facts (the three numbers, install commands) mirror the root
`README.md`; keep them in sync when the main README changes.

`public/board-loop.mp4` is rendered from the Remotion project in `../promo`:

```bash
devenv --profile promo tasks run lific:promo:render
```

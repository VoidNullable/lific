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

The landing page's product images are stills rendered from the Remotion
project in `../promo` and cut into one image per window: `AgentsStill`,
`HumansStill`, `TeamsStill` and `EveryoneSplit` (with `freeze`). Each file's
header has its render and crop commands. They copy the real web UI, so
re-render them when the screens they show change.

`lific:promo:render` still renders the board loop to
`public/board-loop.mp4`, which the page no longer uses.

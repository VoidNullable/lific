# Public project routes

This subtree provides the anonymous, read-only Topcoat surfaces for `/public/{PROJECT}/issues`, `/board`, `/issues/{ISSUE-ID}`, `/pages`, and `/pages/{PAGE-ID}`. `resolve(path)` also returns the two compatibility redirects: `/public/{PROJECT}` (with optional trailing slash) to the issues list, and `/public/{PROJECT}/{ISSUE-ID}` to its issue detail. Unsupported public paths return `None` so the router can render not-found.

`screen(cx, &route)` creates an inert public mount. The root must set the existing `Scope::Public(project)` session before these assets run. The controller verifies that scope, issues GET requests only through `lificSession`, requests public reads with `credentials: 'omit'`, and never uses the private shell/session state. Issue/page content and visible comments use only public DTO fields. Mutations have no rendered form or controls. Attachment thumbnails and downloads use the shared attachment client so browser fetches omit credentials too.

The route controller supports public project indexes, issue and page detail, comments, public attachment links, older-comment paging, and comment/attachment deep links. Unsupported public API methods and private route fallbacks are rejected by the existing scoped session resolver.

Run focused tests with:

```sh
node --test src/topcoat/public/assets/public.test.js
devenv --profile topcoat-e2e shell -- node --test src/topcoat/public/assets/public.browser.test.js
```

The browser suite runs Playwright headlessly.

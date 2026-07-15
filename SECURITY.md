# Security policy

## Supported versions

The latest released version receives security fixes. If a vulnerability affects an older release, upgrade to the latest release before requesting a backport assessment.

## Reporting a vulnerability

Do not open a public issue for an undisclosed vulnerability. Use the repository's private security-advisory flow on GitHub when available. If that flow is unavailable, contact the maintainers through the private contact channel listed in the repository owner profile and include `Lific security report` in the subject.

Include the affected version, deployment type, configuration relevant to the issue, reproduction steps, impact, and any proposed mitigation. Remove API keys, session tokens, passwords, database files, and personal data from the report.

## Security-sensitive deployment areas

Operators should treat the following as trust boundaries:

- API keys, OAuth access tokens, sessions, and unbound shell-minted operator keys;
- `authz_enforced`, `[auth] required`, signup, and browser auto-login settings;
- `server.public_url`, CORS, and `server.trusted_proxies` when running behind a proxy;
- attachment uploads, MIME validation, content-addressed storage, and orphan cleanup; and
- service files, database backups, and any host account with shell access.

Keep auth-optional instances local or firewalled. Configure only proxy ranges you operate. Rotate credentials that appear in logs, shell history, configuration files, or incident reports.

## Disclosure

Maintainers will acknowledge receipt, investigate the report, coordinate a fix and release, and credit reporters when they request it and disclosure is safe.

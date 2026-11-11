# winget

Lific is in the Windows Package Manager community repository as
`VoidNullable.lific`. Users install with:

```powershell
winget install VoidNullable.lific
```

The package installs the release's `lific-windows-x86_64.exe` as a portable
command, so `lific` lands on the PATH with no installer. It depends on the
Microsoft Visual C++ 2015+ runtime, which winget installs first.

A community contributor added it at 2.11.1 in
[microsoft/winget-pkgs#456638](https://github.com/microsoft/winget-pkgs/pull/456638).

## How new versions get there

The `winget` job in `.github/workflows/release.yml` runs after the release job
on every tag. It runs [Komac](https://github.com/russellbanks/Komac) (pinned and
checksum-verified), which:

1. reads the latest `VoidNullable.lific` manifest from winget-pkgs,
2. downloads the new Windows binary and hashes it,
3. writes a new version that keeps every other field (installer type,
   dependency, descriptions, tags) and refreshes the release date and notes,
4. pushes it to a fork of winget-pkgs owned by the token's account (creating
   the fork if needed) and opens a pull request.

Microsoft's validation pipeline and moderators then merge it, usually within a
day or two. Nothing is live for users until that PR merges.

The job is non-fatal (`continue-on-error: true`). Without the `WINGET_TOKEN`
secret it skips with a notice.

## One-time setup

1. Create a **classic** personal access token at
   <https://github.com/settings/tokens> with only the `public_repo` scope, on the
   account that should own the winget-pkgs fork. Fine-grained tokens cannot open
   pull requests against repositories you don't own, so classic is required.
   Give it no expiry, or put the expiry on a calendar.
2. In `VoidNullable/lific` → Settings → Secrets and variables → Actions → New
   repository secret: name `WINGET_TOKEN`, value the token.

## Release checklist

After a release, open the PR link the `winget` job printed and keep an eye on
it. If a moderator asks for changes, answer on the PR.

## Re-running a failed update

Re-run the `winget` job from the release's workflow run in the Actions tab.
Komac refuses to open a duplicate when a PR for the same version is already
open. To do it by hand:

```bash
GITHUB_TOKEN=... komac update VoidNullable.lific --version X.Y.Z \
  --urls https://github.com/VoidNullable/lific/releases/download/vX.Y.Z/lific-windows-x86_64.exe \
  --submit
```

## Stale description

The manifest's description is copied forward from version to version, including
its claims (the MCP tool count and token figure). When those change, edit them
in the generated PR, or run Komac by hand without `--submit`, using `--output`,
and adjust the files before submitting.

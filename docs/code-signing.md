# Code signing policy

Windows releases of ByteForge are signed for free through the SignPath Foundation's open-source
program: free code signing provided by [SignPath.io](https://about.signpath.io/), certificate by
[SignPath Foundation](https://signpath.org/).

> Status: the application to SignPath Foundation is pending. Until it is approved, Windows
> downloads are unsigned and Windows SmartScreen will warn before running them.

## What gets signed

Only builds made by the [Release workflow](../.github/workflows/release.yml) from a `v*` tag of
this repository: `byteforge.exe` (inside the zip and the installer) and the
`ByteForge-<version>-windows-x64-setup.exe` installer. Nothing built locally or by third parties is
signed. Every signing request is approved by hand in SignPath.

## Team

- Committers and reviewers: [DevStarByte](https://github.com/DevStarByte)
- Approvers: [DevStarByte](https://github.com/DevStarByte)

## Privacy

ByteForge sends no telemetry and collects no personal data. It connects to the internet only:

- to fetch the module index and download modules from this repository (or a module registry you
  configure), when you open the Modules view or install or update a module;
- through the tools you run from it, such as `dotnet` (NuGet restore), `git` and `docker`, which
  follow their own privacy policies.

## Setup (maintainers)

1. Apply at <https://signpath.org/apply> with this repository.
2. In SignPath, create the project with two artifact configurations, slugs `exe` and `installer`,
   from [`packaging/windows/signpath/`](../packaging/windows/signpath/), and link the GitHub
   trusted build system to this repository.
3. In GitHub (Settings → Secrets and variables → Actions), add the secret `SIGNPATH_API_TOKEN` (the
   CI user's API token) and the variables `SIGNPATH_ORGANIZATION_ID`, `SIGNPATH_PROJECT_SLUG` and
   `SIGNPATH_POLICY_SLUG` (usually `release-signing`).

From then on, every tagged release is signed. Builds from master pushes stay unsigned.

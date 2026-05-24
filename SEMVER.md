# Versioning Policy

zendriver-rs follows [Semantic Versioning](https://semver.org/).

## Pre-1.0 (0.x.y)

While the major version is 0:
- Minor bumps (0.X.0) MAY include breaking changes.
- Patch bumps (0.x.Y) are non-breaking bug fixes.
- We aim to minimize churn but won't artificially delay improvements.

## Post-1.0

Standard SemVer. Breaking changes require a major bump.

## `#[non_exhaustive]` enums

Enums marked `#[non_exhaustive]` MAY gain variants in minor bumps even
post-1.0 — adding a variant is not considered a breaking change for these
types. As of 0.1.0, this set is: all `*Error` types, `AriaRole`,
`ResourceType`, `AbortReason`.

Other enums (Platform, Channel, ProfileKind, MouseButton, SpecialKey,
DialogType, DownloadProgressState, FetcherPhase, RequestStage, Format,
ClearanceOutcome) are committed-stable; adding variants is a SemVer
break.

## Internal crates

`zendriver-transport` is internal and its API may change in any minor
release without warning. Depend on the `zendriver` crate's re-exports
instead.

## MSRV

We support Rust 1.75 minimum. MSRV bumps follow the same SemVer rules
as API changes — minor bump for MSRV bump.

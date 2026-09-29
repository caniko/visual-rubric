# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.4.0] - 2026-09-30

### Added

- Coverage contracts and reports for declared surfaces, transitions, and exclusions.
- Calibration corpus contracts, deterministic checks, and browser capture tooling.
- All-feature package and documentation checks, and a check using the declared
  minimum Rust version.

### Changed

- The `audit` feature also enables `batch`, matching the audit command's use of
  the batch runner.
- Repository and release workflow ownership moved to GitHub.
- Crate packages explicitly include source, fixtures, README, license, and changelog.

### Fixed

- Direct sequence evaluation honors custom and preset system prompts, matching
  single-image and pipeline evaluation.
- Oversized direct sequences are rejected before reading or encoding frame files.
- Local audit serving reads complete, bounded HTTP request lines, avoiding false
  missing-file responses when a request arrives in multiple TCP reads.

### Security

- Updated the lockfile to rustls 0.23.45 for RUSTSEC-2026-0285 and to the
  non-yanked chacha20 0.10.2 release.

### Removed

- **Breaking:** the public `build_codex_acp_args` re-export. Consumers must use
  the supported configuration and evaluation APIs rather than constructing the
  internal ACP command line through that helper.

## [0.3.0] - 2026-07-16

### Added

- Versioned, content-addressed capture-manifest contracts with root-contained
  artifact validation and canonical JSON serialization.
- Normalized observation, finding, visual-run, and calibration-sentinel
  contracts with stable finding fingerprints and exact capture coverage checks.

### Changed

- Exposed the QA contracts through the public crate API and added SHA-256
  artifact integrity validation.

## [0.2.0] - 2026-06-17

### Added

- Configured subcommand with TOML-based config file support, including `--mode direct` and `--mode pipeline` overrides.
- Vision pipeline mode (Qwen3-VL extraction followed by ACP rubric scoring).
- Generic question presets `ui-regression`, `website-install`, and `manuscript-figure` with standard system prompts, selectable via `--preset` on the `image`, `audit`, and legacy CLI paths.
- `presets::find`, `presets::extend`, and `QuestionPreset::system_prompt` so presets supply a default system prompt when `--system-prompt` is not given and projects can layer their own context on a generic preset; unknown-preset errors now list the available names.
- `Debug`, `non_exhaustive`, `must_use` derives on public types per Rust API guidelines.
- `cargo-deny` configuration with EUPL-1.2 and dependency licenses.

### Changed

- Refactored presets from project-specific to generic with `verdict_schema` macro and `extend` helper.
- Replaced env-var configuration with TOML for the configured subcommand.
- Adopted `percent-encoding` for static path handling.
- Split oversized Rust modules for maintainability.
- Clarified audit and ACP error messages with neutral ACP branding.

### Fixed

- Honor custom `codex-acp` binary arguments.
- Harden viewport dimension and backoff validation.
- Remove fallback `unwrap` calls, propagate errors idiomatically.
- Report batch log capture failures instead of swallowing them.
- Fix release CI on Rust 1.85.
- Auto-fix clippy lints across the codebase.

### Performance

- Avoid cloning classifier error text.
- Preallocate batch report collections.

### Documentation

- Document configured TOML workflow and crate feature flags.

### CI

- Refresh workflows with cross-compilation support and devShell restructuring.

## [0.1.0] - 2026-06-06

### Added

- Initial visual-rubric library and CLI for Codex ACP screenshot rubric checks, static-site auditing, and reusable worker pools.
- Versioned audit reports with aggregate status, capture controls, hosted-path validation, and CI-friendly deterministic test fixtures.

### Changed

- Split CLI, audit, static-server, pool, and crate tests into focused modules while keeping the public API documented.

### Fixed

- Preserve structured rubric anomaly details and keep fake browser test wrappers portable across shell environments.

[Unreleased]: https://github.com/caniko/visual-rubric/compare/0.4.0...HEAD
[0.4.0]: https://github.com/caniko/visual-rubric/compare/0.3.0...0.4.0
[0.3.0]: https://codeberg.org/caniko/visual-rubric/compare/0.2.0...0.3.0
[0.2.0]: https://codeberg.org/caniko/visual-rubric/compare/0.1.0...0.2.0

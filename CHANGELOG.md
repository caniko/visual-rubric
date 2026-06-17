# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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

[Unreleased]: https://codeberg.org/caniko/visual-rubric/compare/0.2.0...HEAD
[0.2.0]: https://codeberg.org/caniko/visual-rubric/compare/0.1.0...0.2.0

# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Registered generic question presets `ui-regression`, `website-install`, and `manuscript-figure` with standard system prompts, selectable via `--preset` on the `image`, `audit`, and legacy CLI paths.
- `presets::find`, `presets::extend`, and `QuestionPreset::system_prompt` so presets supply a default system prompt when `--system-prompt` is not given and projects can layer their own context on a generic preset; unknown-preset errors now list the available names.

## [0.1.0] - 2026-06-06

### Added

- Initial visual-rubric library and CLI for Codex ACP screenshot rubric checks, static-site auditing, and reusable worker pools.
- Versioned audit reports with aggregate status, capture controls, hosted-path validation, and CI-friendly deterministic test fixtures.

### Changed

- Split CLI, audit, static-server, pool, and crate tests into focused modules while keeping the public API documented.

### Fixed

- Preserve structured rubric anomaly details and keep fake browser test wrappers portable across shell environments.

[Unreleased]: https://codeberg.org/caniko/visual-rubric/compare/0.1.0...HEAD

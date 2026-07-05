---
name: visual-rubric
description: Use when working with visual-rubric screenshot review in downstream projects, including configuring, integrating, debugging, validating, or interpreting visual-rubric CLI, audit, pipeline, RubricPool, BatchRubricRun, and preset-based workflows.
---

# visual-rubric

Use `visual-rubric` for AI-assisted screenshot review when deterministic tests cannot fully judge visual completeness, hierarchy, clipping, overlap, contrast, or generated asset quality.

## Discovery First

Before changing code or rerunning checks, inspect the project-local source of truth:

- Existing wrappers, docs, and commands: search for `visual-rubric`, `visual_rubric`, `RubricPool`, `BatchRubricRun`, `--visual-rubric`, `ui-regression`, `website-install`, and `manuscript-figure`.
- Existing config: check `~/.config/visual-rubric/config.toml`, project Home Manager/Nix modules, or caller-specific config flags.
- Existing artifacts: inspect screenshots and reports under `target/visual-rubric/` before judging failures.
- Existing runner conventions: use the project wrapper when one exists instead of bypassing it with raw `visual-rubric` commands.

Never fabricate, synthesize, or silently substitute missing screenshots, reports, config, generated figures, or browser captures. If a required input is missing or invalid, stop and report:

- the missing artifact or source,
- why it is required,
- the upstream producer command or workflow that regenerates it,
- the validation command that proves it is fixed.

## Choosing The Path

- Use `visual-rubric configured` when the host provides `~/.config/visual-rubric/config.toml`.
- Use `visual-rubric image --preset <name> --image <png>` for one-off screenshot checks.
- Use `visual-rubric audit` for local static website audits that need browser capture plus rubric review.
- Use library APIs for embedded runners:
  - `presets::extend` to layer project context on generic preset prompts.
  - `RubricPool` for repeated checks with worker reuse, retry backoff, quota handling, and recycling.
  - `BatchRubricRun` for caller-provided asset batches, changed-file selection, partial-error reports, and log capture.

Prefer project-specific wrappers when present. Raw CLI calls are appropriate only when no wrapper exists or when isolating a visual-rubric failure.

## Presets

- `ui-regression`: application UI screenshots; checks completeness, readability, clipping, overlap, blank regions, contrast, and broken layout.
- `website-install`: project install pages; checks install-section clarity, scanability, call-to-action placement, command-copy ergonomics, and responsive layout.
- `manuscript-figure`: scientific figure PNGs; checks labels, legends, axes, panel assembly, crop margins, contrast, and publication readability.

Project-specific judgment belongs in an appended system prompt/context, not in a new generic preset unless it applies across projects.

## Known Downstream Patterns

- SynDB: manuscript figure reviews produce reports under `target/visual-rubric/manuscript-figures`; use the project `syndb article figure ... --visual-rubric-*` workflow and adapter tests.
- regicide: Rust `xtask` and `visual-test` integrations use `ui-regression` plus game-specific context, often through `~/.config/visual-rubric/config.toml`.
- plinth and rs-modde: website audits call the `visual-rubric` CLI with the `website-install` preset; respect `VISUAL_RUBRIC_BIN` or wrapper flags.
- pink-raven: e2e screenshot wrappers use shared host config and write reports under `target/visual-rubric/`.
- infernix and canix: host/Home Manager modules generate `~/.config/visual-rubric/config.toml`; fix the declaring module rather than hand-editing generated config.

## Validation

Use the smallest validation that proves the visual path is working:

- One screenshot: `visual-rubric configured --json --image <png> --preset ui-regression`.
- Website install page: `visual-rubric audit --root <static-root> --path <path> --preset website-install --report target/visual-rubric/report.json`.
- Embedded Rust caller: run the project wrapper test or command that owns screenshot generation, then inspect both the saved screenshot and report.

When a check fails, preserve the screenshot and report path in the handoff. A visual failure without the artifact is not actionable.

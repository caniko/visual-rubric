# visual-rubric vision

`visual-rubric` runs AI-assisted rubric checks against screenshots through
`codex-acp` and other ACP backends. It is built for local visual UX review
loops where deterministic tests can prove structure and screenshots can catch
layout, hierarchy and readability regressions. It ships a library, a
`visual-rubric` CLI, a calibration corpus, and a skill that tells downstream
agents how to use it.

## Problem

Structural tests pass on pages that look broken: clipped text, overlapping
controls, blank regions, unreadable contrast. A human reviewer catches those,
but not on every commit. `visual-rubric` turns a screenshot and a written
question into a bounded model call with a strict JSON verdict, then adds the
machinery a CI loop needs — process reuse, retries, quota detection, caching,
ordering across interaction steps, and a deterministic gate that does not call
a model at all.

## Principles

- **Deterministic checks first, semantic review second.** The calibration gate
  catches declared DOM, layout and runtime classes without a model call; it
  complements semantic screenshot review and does not replace it.
- **The verdict is a contract.** The model must return strict JSON
  (`verdict`, `reason`, `anomalies`), so callers can branch on a value instead
  of prose.
- **Project judgment stays with the caller.** The default prompt covers only
  generic breakage; project-specific standards arrive as a caller-supplied
  system prompt. The project ships a skill describing that boundary.
- **Ordered journeys are one request.** A sequence is sent as labelled
  multimodal frames with a bounded frame count so the rubric can judge the
  interaction between checkpoints, not just each state.
- **Evaluation has to survive real use.** `RubricPool` reuses processes, backs
  off, detects quota, and recycles workers; `BatchRubricRun` selects changed
  assets, reports partial errors, captures ACP logs and supports issue
  classification hooks.
- **Test plumbing is not a verdict.** `--fake-pass` and the fake ACP helper
  exist to exercise capture and report plumbing and are documented as not a
  semantic visual verdict.
- **Agents get read-only authority.** The evaluator selects an advertised
  read-only or plan mode and denies permission requests rather than granting
  the reviewing model write access.
- **Reports are versioned artifacts.** Audit output is versioned JSON with
  aggregate status, capture URL, elapsed time, effective options and one result
  per screenshot; `--fail-on-rubric` makes a failure a CI failure.

## Scope

- Image rubric evaluation, single and batch, with a caller-provided question
  and system prompt.
- Ordered sequence evaluation with bounded frames and semantic transition
  assessment.
- Direct ACP backends (`codex-acp`, OpenCode `acp`) with model, effort and mode
  negotiation, and a pipeline mode that extracts with Qwen3-VL before scoring.
- Process pooling, retries, quota detection and worker recycling.
- Content-addressed verdict caching and changed-file batch selection.
- A calibration corpus and deterministic checks, plus a Chromium capture
  pipeline that records images, DOM, accessibility, readiness, console and
  layout evidence with a browser matrix.
- Local static hosting (`serve`) and advisory audit reports; versioned coverage
  contracts for declared surfaces, transitions and exclusions.
- A Home Manager-managed `configured` mode reading
  `~/.config/visual-rubric/config.toml`.

## Non-goals

- Replacing unit, integration or snapshot tests; it judges what structure
  cannot express.
- Fabricating, synthesizing or substituting missing screenshots, reports or
  captures; a missing artifact is reported, not improvised.
- Acting as a general screenshot-diffing tool or a design system.
- Giving the reviewing model write access or permission approval.
- Treating a fake or pipeline-only pass as a semantic verdict.

## Direction

Version 0.4.0 added coverage contracts, the calibration corpus with browser
capture, all-feature package checks and the MSRV check, and removed the public
ACP argument-building re-export as a breaking change. Unreleased work is fixing
configured evaluations so ACP launch arguments and model/effort settings
survive, refreshing effort choices after model selection, and selecting an
advertised read-only mode including OpenCode's `plan` mode. Recent features
before that were the capture pipeline with content-addressed verdict cache and
calibration, ordered sequence evaluation, and configuring rubric models
through ACP.

## Place in the fleet

Canix consumes the repository as the `visual-rubric` flake input and runs it
from Infernix on atlas in direct mode against OpenCode; the autonomous QA plan
for Regicide uses it as a gate, and its instructions forbid weakening
visual-rubric, SSIM or baseline rules to obtain green results. The repository
also ships the `visual-rubric` skill used by downstream projects.

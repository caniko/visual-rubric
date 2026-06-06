# visual-rubric

<!-- simit:badges:start -->

[![CI](https://img.shields.io/badge/CI-managed-2088ff)](.forgejo/workflows/ci.yaml) [![Nix](https://img.shields.io/badge/Nix-managed-5277c3)](flake.nix) [![crates.io](https://img.shields.io/badge/crates.io-ready-f46623)](https://crates.io/crates/visual-rubric)

<!-- simit:badges:end -->

`visual-rubric` runs AI-assisted rubric checks against screenshots through
`codex-acp`. It is intended for local visual UX review loops where deterministic
tests can prove structure and screenshots can catch layout, hierarchy, and
readability regressions.

The crate exposes:

- `evaluate_image_rubric_with_options` for one-off screenshot checks.
- `evaluate_image_rubric_with_config` when callers need a custom `codex-acp`
  binary, environment, or working directory.
- `RubricPool` for repeated checks with process reuse, retry backoff, quota
  detection, and worker recycling.
- `visual-rubric` CLI for image checks, local static hosting, screenshot
  capture, and advisory audit reports.

Project-specific judgment belongs in the caller-provided `system_prompt`; the
default prompt only covers generic screenshot breakage such as clipped text,
overlapping controls, blank regions, illegible contrast, and visibly broken
layout.

```sh
visual-rubric \
  --image site-desktop.png \
  --question "Does the install flow make prerequisites, commands, and next steps clear?" \
  --system-prompt "You are auditing a software project website install section."
```

The same form is available as an explicit subcommand:

```sh
visual-rubric image \
  --image site-desktop.png \
  --question "Does the install section stay readable?"
```

For local website iteration, serve a static directory, capture browser
screenshots, and write a report:

```sh
visual-rubric audit \
  --root website/public \
  --path __audit/install.html \
  --browser chromium \
  --browser-arg=--disable-dev-shm-usage \
  --wait-ms 250 \
  --capture-retries 1 \
  --viewport desktop=1440x1100 \
  --viewport mobile=390x1800 \
  --question "Does this install section make the next action obvious?" \
  --report target/visual-rubric/report.json
```

Audit reports are versioned JSON. They include an aggregate status, capture URL,
elapsed time, effective high-level options, and one rubric result per screenshot.
Use `--fail-on-rubric` when CI should fail on rubric failures or rubric errors.

For manual inspection without rubric evaluation:

```sh
visual-rubric serve --root website/public --port 1111
```

The model must return strict JSON:

```json
{ "verdict": "pass", "reason": "short reason", "anomalies": [] }
```

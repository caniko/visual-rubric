# visual-rubric

`visual-rubric` runs AI-assisted rubric checks against screenshots through
`codex-acp`. It is intended for local visual UX review loops where deterministic
tests can prove structure and screenshots can catch layout, hierarchy, and
readability regressions.

The crate exposes:

- `evaluate_image_rubric_with_options` for one-off screenshot checks.
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
  --viewport desktop=1440x1100 \
  --viewport mobile=390x1800 \
  --question "Does this install section make the next action obvious?" \
  --report target/visual-rubric/report.json
```

For manual inspection without rubric evaluation:

```sh
visual-rubric serve --root website/public --port 1111
```

The model must return strict JSON:

```json
{ "verdict": "pass", "reason": "short reason", "anomalies": [] }
```

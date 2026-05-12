# Changelog

All notable public release changes are tracked here.

## 0.1.0-alpha.1 - 2026-05-12

Initial public research preview.

- Added the `chattybara` Rust workspace and CLI.
- Split the public topology: `chattybara` is the TUI chat client, while
  `orca-*` crates hold modem/audio/DSP/frame/link/host/corpus engine work.
- Set chattybara's public home to `https://github.com/nvk/chattybara`.
- Set orca's public engine home to `https://gitlab.com/yokij/orca`.
- Added no-hardware corpus validation, WAV inspection, fixture generation, DSP
  traces, frame classification, and receive-pipeline reports.
- Added the orca packet modem for deterministic lab round trips.
- Added generated TX/RX packet-audio sample sets for clean, loopback, impaired,
  and negative-control receiver tests.
- Added local peer, local node, and TUI chat workflows with `CBAPP/1` app
  envelopes.
- Added station-core scaffolding for multi-mode workspaces, typed
  events/actions, mode capability registry, JSONL replay, fake fixtures,
  receive-only external adapter scaffolds, and TX/reporting safety guards.
- Added reliability simulation for sequence IDs, ACKs, retries, duplicate
  detection, generic fragments, file chunks, hashes, and channel impairment.
- Added real local file transfer plumbing for no-hardware local peer and local
  node tests.
- Added audio device inventory and guarded IC-705 CI-V dry-run/live serial
  paths.
- Added setup, operator, troubleshooting, provenance, virtual lab, and radio
  notes.
- Added local formatting, clippy, tests, and no-hardware lab checks; hosted CI
  configs are optional mirrors.

Known limits:

- No default test requires radio hardware, virtual audio routing, private
  traffic, hosted CI, or live serial/audio access.
- Hardware, live audio routing, and on-air tests are intentionally opt-in and
  operator controlled.

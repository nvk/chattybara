# Changelog

All notable public release changes are tracked here.

## 0.1.0-alpha.8 - 2026-05-13

- Added macOS Keychain-backed Winlink password storage via
  `chattybara winlink account password set`, with stdin and secure prompt
  input paths.
- Live Telnet/CMS sync now reads the configured keychain password when the
  account password source is `keychain`; `CHATTYBARA_WINLINK_PASSWORD` remains
  a temporary override.

## 0.1.0-alpha.7 - 2026-05-13

- Added live Telnet/CMS B2F payload download for supported inbound `FC` and
  `FD` proposals.
- Added B2F binary block checksum validation, B2 LZHUF CRC validation, LZHUF
  and gzip decompression, message header/body parsing, and received attachment
  saving.
- Added guarded live Telnet/CMS outbox sending behind explicit `--allow-send`.
- Existing metadata placeholders are replaced with downloaded messages when
  the same message ID is received later.

## 0.1.0-alpha.6 - 2026-05-13

- Fixed live Telnet/CMS sync prompt handling by consuming full CR-terminated
  login prompts and sending each access-code line as one write.
- Changed the default live CMS endpoint to `cms-z.winlink.org` because
  production CMS rejects unknown client types.
- Fixed the early B2F receive sequence to send `;FW`, the local SID, and `FF`
  before parsing pending inbound proposals.

## 0.1.0-alpha.5 - 2026-05-13

- Changed the default Winlink CMS endpoint to `cms.winlink.org`.
- Send the Telnet access code as `CMSTELNET`.
- Consume the full `Callsign :` and `Password :` prompts before sending login
  lines, avoiding leftover prompt bytes in the B2F handshake reader.

## 0.1.0-alpha.4 - 2026-05-13

- Added receive-only live Telnet/CMS inbox metadata sync for Winlink.
- Added local station settings via `chattybara station config --station CALL`
  so personal call signs stay outside committed examples and source.
- Added secure Winlink challenge response support using an environment-sourced
  password, without command-line password flags.
- Added fake CMS regression tests for Telnet login, B2F handshake, proposal
  checksum handling, metadata storage, and safe payload deferral.
- Documented the live inbox check workflow and its current metadata-only limit.

## 0.1.0-alpha.3 - 2026-05-13

- Added an early transport-neutral Winlink mailbox crate and CLI workflow.
- Added local Winlink account/store setup, compose/read/list, fake sync, and
  B2F proposal reporting for no-radio testing.
- Added guarded Telnet/CMS dry-run connectivity checks plus VARA and orca
  transport status surfaces.
- Added Winlink mode registry entries and `/workspace winlink` TUI workspace
  selection.

## 0.1.0-alpha.2 - 2026-05-12

- Fixed the TUI top status bar so `/station CALL` immediately shows the pending
  setup station before `/start`.

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

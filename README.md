# chattybara

chattybara is a terminal radio chat client with pluggable modem and protocol
backends. It owns the operator experience: setup, station identity, chat
transcript, beacon/CQ monitor, mailbox, file offers, session logs, and backend
selection.

The modem engine used by the current native backends is `orca`. Orca is a
separate project at `https://gitlab.com/yokij/orca` and owns modem, DSP,
audio, frame, fixture, corpus, host, and protocol-lab work. Chattybara consumes
orca as an engine dependency; it does not carry protocol-specific external
modem lab material.

## Status

Current release: `0.1.0-alpha.3`.

This is a public alpha for no-hardware chat-client development. It is useful
for TUI workflow testing, local peer/node sessions, mailbox and file-offer
flows, audio/radio setup plumbing, deterministic packet-audio loopbacks, and
offline regression checks.

Included:

- `chattybara` CLI and TUI chat client.
- Guided setup-oriented `chattybara chat tui` entry point.
- Fake, native loopback, WAV loopback, and localhost node chat backends.
- Clean app envelope for beacon, CQ, mailbox, file offers, fragments, receipts,
  and file chunks.
- ACK/retry simulation, duplicate detection, sequence IDs, timestamps, and
  delivery states.
- Real local file chunking/reassembly for no-hardware local peer and local node
  tests.
- Host audio device inventory and guarded live audio modem plumbing.
- IC-705 profile validation, guarded CI-V frame construction, and opt-in serial
  I/O.
- Generic Hamlib `rigctld` CAT/PTT client and reusable radio/audio profiles.
- Station mode registry for future chat, weak-signal, CW, spot-monitor, and
  operator-console workspaces.
- Early Winlink mailbox workflow: local account/store, compose/read/list,
  deterministic fake sync, B2F proposal modeling, Telnet/CMS dry-run checks,
  and guarded VARA/orca transport status surfaces.
- Local format, clippy, tests, and no-hardware lab checks.

Not included:

- crates.io publication.
- Default tests that require radio hardware, virtual audio cables, private
  traffic, hosted CI, or live serial/audio access.
- Protocol-specific compatibility claims. Those belong in backend-specific
  projects and documented release notes.

## Repositories

- chattybara public chat client: `https://github.com/nvk/chattybara`
- orca modem engine and protocol lab: `https://gitlab.com/yokij/orca`

## Install From Source

```sh
git clone https://github.com/nvk/chattybara.git
cd chattybara
cargo install --path crates/chattybara-cli --locked --force
chattybara --help
chattybara chat tui
```

For development without installing:

```sh
cargo run -p chattybara-cli -- --help
cargo run -p chattybara-cli -- chat tui
cargo run -p chattybara-cli -- chat tui --setup-preview
```

If Cargo cannot write to the default user cache on this workstation, prefix
commands with `CARGO_HOME=$PWD/.cargo-home`. The repo ignores that local cache.

## Common Development Commands

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo run -p chattybara-cli -- lab run out/lab
cargo run -p chattybara-cli -- lab snapshot out/lab/lab-report.json --out out/lab/lab-snapshot.json
cargo run -p chattybara-cli -- lab compare out/lab/lab-snapshot.json out/lab/lab-report.json
cargo run -p chattybara-cli -- audio devices --sample-rate 8000 --channels 1
cargo run -p chattybara-cli -- modem roundtrip "hello chattybara"
cargo run -p chattybara-cli -- modem samples "hello chattybara" out/modem-samples --overwrite
cargo run -p chattybara-cli -- simulate app-link --payload-bytes 180 --drop-first-attempt --duplicate-deliveries
cargo run -p chattybara-cli -- station modes
cargo run -p chattybara-cli -- chat fake-script docs/chat/basic-qso.txt --station JA1TST
cargo run -p chattybara-cli -- chat app-script docs/chat/app-features-basic.txt --station JA1TST
cargo run -p chattybara-cli -- chat local-peer-script docs/chat/local-peer-basic.txt --station-a JA1TST --station-b JA1QSO --out-dir out/local-peer --overwrite
cargo run -p chattybara-cli -- chat local-node-script docs/chat/local-node-app-b.txt --station JA1QSO --peer JA1TST --listen 127.0.0.1:0 --ready-file out/local-node-app.ready --out-dir out/local-node-app-b --overwrite
cargo run -p chattybara-cli -- chat local-node-script docs/chat/local-node-app-a.txt --station JA1TST --peer JA1QSO --connect "$(cat out/local-node-app.ready)" --out-dir out/local-node-app-a --overwrite
cargo run -p chattybara-cli -- chat tui
```

## Current CLI Surface

- `chattybara chat tui` starts the interactive terminal chat UI with guided
  setup, transcript, monitor, mailbox, file-offer list, composer, status, and
  context help.
- `chattybara chat fake-script` runs deterministic no-hardware chat scripts.
- `chattybara chat app-script` exercises app-layer beacon, CQ, mailbox, and
  file-offer state.
- `chattybara chat local-peer-script` runs two local stations over localhost
  packet-audio frames.
- `chattybara chat local-node-script` runs one station per process over
  localhost packet-audio frames.
- `chattybara chat parse-log`, `compare-script-log`, `compare-peer-logs`, and
  `compare-session-logs` validate normalized public chat logs and generated
  session logs.
- `chattybara modem roundtrip`, `encode`, `decode`, `sweep`, `samples`, and
  `live-audio` expose the generic native modem backend tools used by the chat
  client.
- `chattybara audio devices`, `chunks`, and `loopback` inspect and exercise
  local audio plumbing.
- `chattybara rig ic705`, `rig profile`, and `rig hamlib` provide dry-run-first
  radio setup and guarded live control.
- `chattybara station modes`, `fake-events`, `replay`, `guard`, and `external`
  expose the station-core registry and safety gates for future modes.
- `chattybara winlink account`, `compose`, `inbox`, `outbox`, `read`, `sync`,
  `telnet`, and `transport` expose the no-radio Winlink mailbox workflow and
  guarded Telnet/CMS, VARA, and orca transport surfaces.
- `chattybara lab run`, `snapshot`, and `compare` run no-hardware release
  checks.

## TUI Basics

Start with:

```sh
chattybara chat tui
```

Useful commands inside the TUI:

- `/station CALL`
- `/backend fake`
- `/backend native-loopback`
- `/backend native-wav-loopback`
- `/backend native-local-node`
- `/peer CALL`
- `/listen HOST:PORT`
- `/connect-node HOST:PORT`
- `/connect CALL`
- `/send text`
- `/beacon text`
- `/cq text`
- `/mail CALL subject | body`
- `/file-offer CALL filename byte-count sha256 note`
- `/save-session path`
- `/quit`

Keyboard:

- `Tab` and `Shift-Tab` move panes.
- `Enter` activates setup or selected mailbox/file-offer items.
- `?` or `F1` shows context help.
- `Ctrl-Q` exits.

## License

MIT.

Copyright (c) 2026 nvk.

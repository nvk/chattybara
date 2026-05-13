# Station Core And Mode Workspaces

`chattybara-station` is the shared station layer for multi-mode work. The
library itself does not open radios, audio devices, or external apps. The CLI
adds guarded live adapter commands on top of those station events. The station
layer provides:

- station profile
- typed station events and actions
- mode identifiers and capability flags
- workspace identifiers
- transmit/reporting safety guards
- deterministic JSONL event logs
- replay summaries
- fake fixture events for planned modes

## Commands

Save the local operator station outside the repository:

```sh
chattybara station config --station CALL
```

List the mode registry:

```sh
chattybara station modes
```

Generate no-hardware fixture events:

```sh
chattybara station fake-events --mode js8call --station JA1TST --out out/js8/events.jsonl --session-dir out/js8/session
chattybara station replay out/js8/events.jsonl
```

Run the protocol suite:

```sh
chattybara station protocol-suite --station JA1TST --out-dir out/protocol-suite
```

This writes replayable fixture sessions for:

- `js8call-external`: TCP JSON-lines API, default `127.0.0.1:2442`.
- `wsjtx-external`: WSJT-X/FT8 UDP reporting, default
  `127.0.0.1:2237`.
- `fldigi-external`: XML-RPC, default `127.0.0.1:7362`.
- `cw-assist`: receive-only Morse/plain-text fixture decoder.
- `pskreporter`: receive-only HTTPS query adapter.
- `winlink-vara`: transport-status scaffold for future external VARA sync.
- `winlink-orca`: transport-status scaffold for future native orca sync.

Check safety gates:

```sh
chattybara station guard --action send-message
chattybara station guard --action send-message --arm-tx
chattybara station guard --action report-spot --enable-reporting
```

Inspect adapter defaults:

```sh
chattybara station external --adapter js8call
chattybara station external --adapter wsjtx
chattybara station external --adapter fldigi
chattybara station external --adapter cw-assist
chattybara station external --adapter pskreporter
```

Run live or fixture adapters:

```sh
chattybara station external --adapter js8call --live
chattybara station external --adapter wsjtx --live
chattybara station external --adapter fldigi --live
chattybara station external --adapter pskreporter --live
chattybara station external --adapter cw-assist --fixture cw.txt --out out/cw-events.jsonl
```

Transmit paths are guarded. JS8Call and fldigi sends require both
`--enable-tx` and `--allow-transmit`, plus the adapter-specific message fields:

```sh
chattybara station external \
  --adapter js8call \
  --live \
  --enable-tx \
  --allow-transmit \
  --send-to CALL \
  --message "hello"
```

## Workspaces

The TUI still starts with:

```sh
chattybara chat tui
```

Internally, chat is now the first workspace. The planned workspace set is:

- `chat`: transcript, CQ/beacon monitor, mailbox, file offers, composer.
- `weak-signal`: WSJT-X/FT8-style decode table and exchange/log actions.
- `cw-assist`: Morse copy, speed/confidence, keyer/macro controls.
- `spots`: PSK Reporter queue/query/report state.
- `operator-console`: fldigi-style RX/TX text and macros.
- `winlink`: mailbox, outbox, attachments, sync status, and transport
  selection for Telnet/CMS, external VARA, and experimental orca links.
- `rig-setup`: audio, Hamlib, IC-705, HAM Radio Apps, dry-run/live/PTT safety.

The current TUI accepts:

```text
/workspace chat
/workspace weak-signal
/workspace cw-assist
/workspace spots
/workspace operator-console
/workspace winlink
/workspace rig-setup
```

The workspace command is scaffolded so the UI can evolve without changing the
user entry point. `/workspace winlink` is accepted and currently points at the
Winlink mailbox command workflow described in `docs/winlink.md`.

## Safety

Default state is `DRY RUN`. Transmit-capable station actions fail unless TX is
armed. External reporting actions fail unless reporting is enabled. Live
adapters should preserve this rule even when the external application itself can
transmit.

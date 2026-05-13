# Station Core And Mode Workspaces

`chattybara-station` is the shared station layer for multi-mode work. It does
not open radios, audio devices, or external apps. It provides:

- station profile
- typed station events and actions
- mode identifiers and capability flags
- workspace identifiers
- transmit/reporting safety guards
- deterministic JSONL event logs
- replay summaries
- fake fixture events for planned modes

## Commands

List the mode registry:

```sh
chattybara station modes
```

Generate no-hardware fixture events:

```sh
chattybara station fake-events --mode js8call --station JA1TST --out out/js8/events.jsonl --session-dir out/js8/session
chattybara station replay out/js8/events.jsonl
```

Check safety gates:

```sh
chattybara station guard --action send-message
chattybara station guard --action send-message --arm-tx
chattybara station guard --action report-spot --enable-reporting
```

Inspect receive-only external adapter scaffolds:

```sh
chattybara station external --adapter js8call
chattybara station external --adapter wsjtx
chattybara station external --adapter fldigi
chattybara station external --adapter pskreporter
```

These scaffolds do not open network connections yet. They document endpoint
defaults and keep TX/reporting disabled unless an operator explicitly opts in.

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

# Architecture

chattybara is the user-facing chat client. orca is the separate modem engine
used by chattybara's current native backends. The canonical orca repository is
`https://gitlab.com/yokij/orca`.

## Product Boundary

chattybara owns operator workflows:

- terminal chat UI
- setup and profile selection
- station identity
- transcript
- beacon and CQ monitor
- mailbox
- file offers and received-file workflow
- session logs and artifacts
- backend selection
- radio and audio setup surfaces

orca owns engine and lab mechanics:

- audio buffers and WAV utilities
- DSP analysis and channel simulation
- packet frames and decode reports
- link/session state
- host command modeling
- corpus tooling
- no-hardware fixture generation and verification
- backend/protocol research outside the chat-client boundary

The installed chat client binary remains `chattybara`. The orca engine also
ships as its own `orca` binary from the separate public orca repository.
This repository consumes orca engine crates from that public repository as
pinned git dependencies.

## Current Crates

- `chattybara-cli`: chat client binary, command surface, TUI, radio/audio setup commands.
- `chattybara-chat`: app model for sessions, transcript, mailbox, file offers.
- `chattybara-station`: station profile, typed events/actions, mode
  capabilities, safety guards, event-log replay, and mode registry data.

Orca crates are not workspace members here. The `chattybara-cli` crate depends
on `orca-audio`, `orca-corpus`, `orca-dsp`, `orca-frames`, and
`orca-host` from `https://gitlab.com/yokij/orca`.

## Station And Mode Direction

The chat client treats every transport or protocol as a mode adapter under the
same station contract:

- `fake`: local-only app state for tests and demos.
- `native-loopback`: in-memory or WAV packet loopback.
- `native-local-node`: two-process localhost packet-audio link.
- external-app scaffolds: JS8Call JSON, WSJT-X UDP, fldigi XML-RPC, and PSK
  Reporter query/reporting surfaces.
- Winlink mailbox adapters: Telnet/CMS for no-radio Internet sync first, then
  external VARA modem sessions and experimental orca sessions behind the same
  mailbox/store-forward workflow.
- future adapters: KISS-style packet transports, documented modem APIs,
  receive-only decoders, and other protocol stacks.

Mode adapters report capability flags to the TUI: free text, directed message,
conversation, ARQ, file transfer, mailbox, store-forward, fixed time slots,
decode table, spot reporting, logging, external app API, native modem, RX-only,
time-sync requirement, and transmit capability. The TUI should surface those
flags before an operator starts a session.

The station core is intentionally broader than chat. FT8/WSJT-X belongs in a
weak-signal decode/exchange workspace, Morse/CW in an assist workspace, PSK
Reporter in a spot monitor, and fldigi in an operator console. The existing chat
surface remains the default workspace.

Winlink belongs in a mailbox workspace rather than the free-text chat surface.
The mailbox model should be transport-neutral: Internet Telnet/CMS, external
VARA, and orca-backed experimental links should share the same message store,
outbox, attachment handling, receipts, and safety indicators.

## Release Principle

The public release should be a fully usable no-hardware chat application first.
On-air modem compatibility remains a separate, backend-specific milestone.

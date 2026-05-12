# chattybara chat

`chattybara chat` is the native chat application. It owns the operator-facing
TUI, transcript, monitor, mailbox, file offers, logs, setup workflow, and
backend selection. Modem/audio/link details live behind backend adapters.

Run the checked-in no-hardware flows:

```sh
cargo run -p chattybara-cli -- chat fake-script docs/chat/basic-qso.txt --station JA1TST
cargo run -p chattybara-cli -- chat app-script docs/chat/app-features-basic.txt --station JA1TST
cargo run -p chattybara-cli -- chat local-peer-script docs/chat/local-peer-basic.txt --station-a JA1TST --station-b JA1QSO --out-dir out/local-peer --overwrite
cargo run -p chattybara-cli -- chat local-peer-script docs/chat/local-peer-app-features.txt --station-a JA1TST --station-b JA1QSO --out-dir out/local-peer-app --overwrite
cargo run -p chattybara-cli -- simulate app-link --payload-bytes 180 --drop-first-attempt --duplicate-deliveries
cargo run -p chattybara-cli -- chat local-node-script docs/chat/local-node-a.txt --station JA1TST --peer JA1QSO --listen 127.0.0.1:0 --ready-file out/local-node.ready --out-dir out/local-node-a --overwrite
cargo run -p chattybara-cli -- chat local-node-script docs/chat/local-node-b.txt --station JA1QSO --peer JA1TST --connect "$(cat out/local-node.ready)" --out-dir out/local-node-b --overwrite
cargo run -p chattybara-cli -- chat tui
```

Backend families:

- `fake`: local-only app state for tests and demos.
- `native-loopback`: in-memory packet loopback through the native modem engine.
- `native-wav-loopback`: WAV encode/read/decode loopback through the native
  modem engine.
- `native-local-node`: two-process localhost packet-audio link.
- future adapters: documented external applications, KISS-style transports,
  receive-only spot monitors, and other modem/protocol stacks.

## Fake Script

Script commands:

- `CONNECT <call>`
- `SEND <text>`
- `RX <call> <text>`
- `STATUS`
- `DISCONNECT`

Use synthetic calls and payloads in checked-in examples.

## App Feature Script

`chat app-script` models user-visible chat application features independently of
any backend wire format. It is intended for UI, persistence, and workflow tests
that need beacon/CQ/mailbox/file-transfer metadata before those features are
connected to a live transport.

App script commands:

- `BEACON <text>` records a station beacon.
- `CQ <text>` records a CQ call.
- `MAIL <to-call> <subject> | <body>` records a mailbox message.
- `FILE-OFFER <to-call> <filename> <byte-count> <sha256> [note]` records file metadata only.
- `STATUS` records current app feature counts.

## Local Peer Script

`chat local-peer-script` runs two independent station states over a localhost TCP
audio-frame link. Each `CONNECT`, `SEND`, and `DISCONNECT` command is encoded
with the native packet modem, sent as audio samples over the local link, decoded
by the opposite station, and recorded in both transcripts.

Script commands:

- `A CONNECT` connects station A to station B.
- `A SEND <text>` sends text from station A to station B.
- `B SEND <text>` sends text from station B to station A.
- `A BEACON <text>` sends a packetized app beacon from station A.
- `A CQ <text>` sends a packetized CQ call from station A.
- `A MAIL <subject> | <body>` sends packetized mailbox metadata to station B.
- `A FILE-OFFER <filename> <byte-count> <sha256> [note]` sends packetized file metadata to station B.
- `A FILE-SEND <path> [note]` sends a real local file as hashed chunks.
- `A DISCONNECT` disconnects both stations after a packetized disconnect frame.
- `A STATUS` or `B STATUS` records a local status event without sending a frame.

App feature frames use a clean text envelope:

```text
CBAPP/1
{"kind":"mail","from":"JA1TST","to":"JA1QSO","id":"JA1TST-00000001","sequence":1,"timestamp_ms":1,"delivery":"sent","ack_required":true,"subject":"Test","body":"Body"}
```

Large app payloads are split into `fragment` or `file-chunk` packets with
per-chunk and full-payload SHA-256 checks. The decoder still accepts the older
local `APP-*` packet strings for legacy test artifacts.

Use `--gain`, `--snr-db`, and `--drift-ppm` to apply deterministic channel
impairments to outbound packet audio before decode.

## Local Node Script

`chat local-node-script` runs one station per process. One side listens on
localhost and writes a `--ready-file`; the second side connects to that address.
Both sides run their own scripts and exchange the same packetized audio frames
used by `local-peer-script`.

Node script commands:

- `CONNECT`
- `EXPECT-CONNECT`
- `SEND <text>`
- `EXPECT-MSG <text>`
- `BEACON <text>`
- `EXPECT-BEACON <text>`
- `CQ <text>`
- `EXPECT-CQ <text>`
- `MAIL <subject> | <body>`
- `EXPECT-MAIL <subject> | <body>`
- `FILE-OFFER <filename> <byte-count> <sha256> [note]`
- `EXPECT-FILE-OFFER <filename> <byte-count> <sha256> [note]`
- `FILE-SEND <path> [note]`
- `EXPECT-FILE-SEND <filename> <byte-count> <sha256> [note]`
- `DISCONNECT`
- `EXPECT-DISCONNECT`
- `STATUS`

## TUI Chat

`chat tui` starts an interactive terminal chat surface with guided setup/radio,
transcript, beacon/CQ monitor, mailbox, file-offer list, status line, and context
actions.

Session commands:

- `/station <call>`
- `/backend <fake|native-loopback|native-wav-loopback|native-local-node>`
- `/peer <call>`
- `/listen <host:port>`
- `/connect-node <host:port>`
- `/connect <call>`
- `/send <text>`
- `/rx <call> <text>`
- `/disconnect`
- `/status`
- `/quit`

App commands:

- `/beacon <text>`
- `/cq <text>`
- `/mail <call> <subject> | <body>`
- `/mail-read <seq>`
- `/mail-reply <seq> <subject> | <body>`
- `/file-offer <call> <filename> <byte-count> <sha256> [note]`
- `/file-inspect <seq>`
- `/file-accept <seq> <dir>`
- `/app-status`
- `/save-app <path>`

Artifact commands:

- `/save-log <path>`
- `/save-artifacts <path>`
- `/save-session <dir>`

Keyboard navigation:

- `Tab` and `Shift-Tab` move focus through transcript, monitor, mailbox, files, and composer panes.
- `Up` and `Down` move selected mailbox or file-offer items when those panes are focused.
- `Enter` starts guided setup or opens selected mailbox/file-offer items when the composer is empty.
- `?` and `F1` append context help.
- `Esc` clears the composer and returns focus to setup or composer.
- `Ctrl-Q`, `/quit`, or `Ctrl-C` exits.

## Simple Observed Logs

`chat parse-log` turns a normalized public chat log into a transcript JSON
report. `chat compare-script-log` compares that message flow to a chattybara
script. `chat compare-peer-logs` compares two logs from opposite sides of the
same session and checks that each send on one side is the matching receive on
the other.

Simple log commands:

- `OUT <to-call> <text>` records a locally sent message.
- `IN <from-call> <text>` records a received message.

`chat compare-session-logs` resolves generated `chat.log` files from
`local-peer-script` or `local-node-script` session artifacts and runs the same
peer-log comparator.

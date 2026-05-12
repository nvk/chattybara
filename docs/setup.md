# Setup

This repo is designed to work without radio hardware. Start with the no-hardware
checks before connecting audio devices, serial ports, or a transmitter.

## Workstation

Install Rust `1.95` or newer. Linux systems also need the audio and serial
development packages used by `cpal` and `serialport`:

```sh
sudo apt-get update
sudo apt-get install -y --no-install-recommends pkg-config libasound2-dev libudev-dev
```

Install the CLI from a checkout:

```sh
cargo install --path crates/chattybara-cli --locked --force
chattybara --help
```

Run the no-hardware development checks:

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo run -p chattybara-cli -- audio devices --sample-rate 8000 --channels 1
cargo run -p chattybara-cli -- modem roundtrip "hello chattybara"
cargo run -p chattybara-cli -- simulate app-link --payload-bytes 180 --drop-first-attempt --duplicate-deliveries
```

If Cargo cannot write to the default user cache on this workstation, prefix
commands with `CARGO_HOME=$PWD/.cargo-home`.

## No-Hardware Chat

Use local peer for one-process packet audio:

```sh
cargo run -p chattybara-cli -- chat local-peer-script docs/chat/local-peer-basic.txt --station-a JA1TST --station-b JA1QSO --out-dir out/local-peer --overwrite
```

Use local node for two-process packet audio:

```sh
cargo run -p chattybara-cli -- chat local-node-script docs/chat/local-node-a.txt --station JA1TST --peer JA1QSO --listen 127.0.0.1:0 --ready-file out/node.ready --out-dir out/node-a --overwrite
cargo run -p chattybara-cli -- chat local-node-script docs/chat/local-node-b.txt --station JA1QSO --peer JA1TST --connect "$(cat out/node.ready)" --out-dir out/node-b --overwrite
```

Use the TUI after the script path is green. The bare command opens setup with a
no-hardware loopback backend active:

```sh
cargo run -p chattybara-cli -- chat tui
```

The TUI opens on a guided setup/radio pane with the safety state visible. Use
Tab/Shift-Tab to move panes, Enter to start setup or open selected mail/files,
`?` for help, and Ctrl-Q or `/quit` to exit. Slash commands are still available:
`/station CALL`, `/backend native-loopback`, `/peer CALL`, `/listen`,
`/connect-node HOST:PORT`, `/audio-input DEVICE`, `/audio-output DEVICE`,
`/radio-hamlib HOST:PORT`, and `/start`. The old explicit flags still work for
repeatable scripted runs.

## Generic Radio and USB Audio Dry Run

Use Hamlib `rigctld` for common-radio CAT/PTT control and CPAL device names for
USB audio:

```sh
cargo run -p chattybara-cli -- audio devices --include-supported --sample-rate 48000 --channels 1
cargo run -p chattybara-cli -- rig profile --model IC-7300 --input-device "USB Audio CODEC" --output-device "USB Audio CODEC" --out out/radio.toml
cargo run -p chattybara-cli -- rig validate out/radio.toml
cargo run -p chattybara-cli -- rig hamlib status --host 127.0.0.1:4532
cargo run -p chattybara-cli -- modem live-audio "hello radio audio" --sample-rate 48000 --input-device "USB Audio CODEC" --output-device "USB Audio CODEC"
```

`modem live-audio` is dry-run by default. Live packet audio requires
`--live --allow-transmit-audio`. Hamlib PTT TX requires `--allow-transmit`.

## IC-705 Dry Run

```sh
cargo run -p chattybara-cli -- rig ic705 profile --out out/ic705.toml
cargo run -p chattybara-cli -- rig ic705 validate out/ic705.toml
cargo run -p chattybara-cli -- rig ic705 civ --operation read-frequency
cargo run -p chattybara-cli -- rig ic705 civ-serial --operation read-frequency --port /dev/not-opened
```

`civ-serial` does not open a port unless `--live` is passed. Live `ptt-tx`
requires `--allow-transmit`.

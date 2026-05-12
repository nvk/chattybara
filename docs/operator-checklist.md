# Operator Checklist

Use this checklist before any test that can touch radio hardware or external
modem software.

## Before Hardware

- Run `cargo test --workspace`.
- Run `cargo run -p chattybara-cli -- audio devices --sample-rate 8000 --channels 1`.
- Run `cargo run -p chattybara-cli -- simulate app-link --drop-first-attempt --duplicate-deliveries`.
- Confirm scripts pass in `fake`, `local-peer`, and `local-node` modes.
- Confirm generated logs contain only synthetic or operator-approved public test content.

## Before IC-705 Serial

- Generate and validate the IC-705 profile.
- Verify the serial port name belongs to CI-V Port A.
- Run `rig ic705 civ-serial` without `--live` first.
- Use `--live` only for receive-safe operations such as `read-frequency` until the station setup is confirmed.
- Use `ptt-tx --live --allow-transmit` only into a dummy load or non-radiating test path.

## Before Hamlib and USB Audio

- Start `rigctld` for the exact radio model and serial device.
- Run `rig hamlib status --host 127.0.0.1:4532` before any PTT command.
- Run `audio devices --include-supported --sample-rate 48000 --channels 1` and copy exact device names.
- Generate and validate a generic `rig profile`.
- Run `modem live-audio` without `--live` first.
- Use `--live --allow-transmit-audio` and `--key-ptt` only into a dummy load or non-radiating test path.

## Before On-Air Tests

- Confirm band, mode, power, and station identification requirements.
- Keep test messages short and non-sensitive.
- Record the exact command, profile, audio devices, frequency, and time.
- Save generated `session.json`, `artifacts.json`, packet WAVs, and logs.
- Stop immediately on unexpected PTT, high ALC, decode instability, or peer confusion.

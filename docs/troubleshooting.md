# Troubleshooting

## Cargo Cache Permission Errors

Use a repo-local cache:

```sh
CARGO_HOME=$PWD/.cargo-home cargo test --workspace
```

The repo ignores `.cargo-home/`.

## No Audio Devices Appear

Run:

```sh
cargo run -p chattybara-cli -- audio devices --include-supported --sample-rate 8000 --channels 1
```

If `devices` is empty, check OS microphone/speaker privacy permissions and USB
driver installation. If devices appear but `supports_requested_config` is false,
try the device default sample rate shown in the report.

## Local Node Test Hangs

Use `--ready-file` on the listener and start the connector only after the file
exists:

```sh
cargo run -p chattybara-cli -- chat local-node-script listener.txt --station JA1QSO --peer JA1TST --listen 127.0.0.1:0 --ready-file out/node.ready --out-dir out/listener --overwrite
cargo run -p chattybara-cli -- chat local-node-script connector.txt --station JA1TST --peer JA1QSO --connect "$(cat out/node.ready)" --out-dir out/connector --overwrite
```

If decode fails, retry without `--snr-db` or `--drift-ppm`, then add impairment
back after the clean path works.

## File Transfer Fails

- Confirm the sender uses `FILE-SEND <path> [note]`.
- Confirm the receiver uses `EXPECT-FILE-SEND <filename> <byte-count> <sha256> [note]`.
- Check `session.json` for `received_files`.
- Check `packets/` for the file offer and file chunk WAVs.
- If the payload is large, start with a small text file until the path is stable.

## IC-705 Serial Does Not Open

- Run `rig ic705 civ-serial` without `--live` to verify frame construction.
- Check the serial port path and OS permissions.
- Verify the CI-V baud rate. The CLI default is `19200`.
- `ptt-tx --live` fails unless `--allow-transmit` is present.

## Hamlib Rigctld Does Not Respond

- Confirm `rigctld` is running and listening on the same host/port passed to `--host`.
- Test receive-safe commands first: `rig hamlib get-frequency`, `get-mode`, and `status`.
- If status reads frequency but not mode or PTT, the radio backend may not support that command.
- `rig hamlib ptt-tx` fails before opening a network connection unless `--allow-transmit` is present.

## Live Audio Does Not Decode

- Run `modem live-audio` without `--live` first and confirm the packet duration and sample rate.
- Use exact device names from `audio devices --include-supported`.
- Match `--sample-rate` and `--channels` to a supported device config, commonly `48000` and `1` for USB radio audio.
- Start with low `--tx-gain`, verify ALC, and increase only after the path is stable.
- If using PTT, confirm `rig hamlib ptt-rx` works before `--key-ptt`.

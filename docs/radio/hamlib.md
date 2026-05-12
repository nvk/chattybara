# Hamlib Rigctld and USB Audio

chattybara can use Hamlib's `rigctld` TCP interface for common-radio CAT/PTT
control and CPAL for host USB audio devices. The default workflow is still
dry-run-first.

Start `rigctld` outside chattybara for your radio. Examples:

```sh
rigctld -m 3085 -r /dev/cu.usbserial-IC705 -s 19200 -t 4532
rigctld -m 3073 -r /dev/cu.usbserial-IC7300 -s 19200 -t 4532
```

Then inspect and validate from chattybara:

```sh
cargo run -p chattybara-cli -- rig hamlib status --host 127.0.0.1:4532
cargo run -p chattybara-cli -- rig hamlib get-frequency --host 127.0.0.1:4532
cargo run -p chattybara-cli -- rig hamlib get-mode --host 127.0.0.1:4532
cargo run -p chattybara-cli -- rig hamlib ptt-rx --host 127.0.0.1:4532
```

PTT TX is guarded:

```sh
cargo run -p chattybara-cli -- rig hamlib ptt-tx --host 127.0.0.1:4532 --allow-transmit
```

Create a generic radio/audio profile:

```sh
cargo run -p chattybara-cli -- audio devices --include-supported --sample-rate 48000 --channels 1
cargo run -p chattybara-cli -- rig profile --model IC-7300 --input-device "USB Audio CODEC" --output-device "USB Audio CODEC" --out out/radio.toml
cargo run -p chattybara-cli -- rig validate out/radio.toml
```

Prepare a live-audio modem run without opening devices:

```sh
cargo run -p chattybara-cli -- modem live-audio "hello radio audio" --sample-rate 48000 --input-device "USB Audio CODEC" --output-device "USB Audio CODEC"
```

The live path opens both audio devices, plays one packet, records RX audio, and
attempts a decode only when explicitly armed:

```sh
cargo run -p chattybara-cli -- modem live-audio "hello radio audio" \
  --sample-rate 48000 \
  --input-device "USB Audio CODEC" \
  --output-device "USB Audio CODEC" \
  --hamlib-host 127.0.0.1:4532 \
  --key-ptt \
  --live \
  --allow-transmit-audio
```

Keep RF power low, use a dummy load or lab-safe path for first tests, and verify
audio drive/ALC before any over-the-air experiment.

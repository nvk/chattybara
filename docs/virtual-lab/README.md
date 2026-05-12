# virtual lab

The default lab path is no-hardware and offline-first. It exercises chattybara's
generic chat, audio, packet-loopback, and station workflows without requiring a
radio, virtual audio cable, hosted CI, or live serial/audio access.

## No-Hardware Commands

```sh
cargo run -p chattybara-cli -- fixture synth out/tone.wav --kind tone-burst --frequency 1000
cargo run -p chattybara-cli -- fixture suite out/regression-suite
cargo run -p chattybara-cli -- corpus verify out/regression-suite
cargo run -p chattybara-cli -- lab run out/lab
cargo run -p chattybara-cli -- lab snapshot out/lab/lab-report.json --out out/lab/lab-snapshot.json
cargo run -p chattybara-cli -- lab compare out/lab/lab-snapshot.json out/lab/lab-report.json
cargo run -p chattybara-cli -- audio chunks --frames 256 out/tone.wav
cargo run -p chattybara-cli -- audio devices --sample-rate 8000 --channels 1
cargo run -p chattybara-cli -- audio loopback --latency-frames 80 --gain 0.8 out/tone.wav out/tone-loopback.wav
cargo run -p chattybara-cli -- simulate channel --gain 0.8 --snr 24 out/tone.wav out/tone-noisy.wav
cargo run -p chattybara-cli -- frames classify out/tone-noisy.wav
cargo run -p chattybara-cli -- frames pipeline out/tone-noisy.wav
cargo run -p chattybara-cli -- modem roundtrip "hello chattybara"
cargo run -p chattybara-cli -- modem encode "hello chattybara" out/packet.wav
cargo run -p chattybara-cli -- modem decode out/packet.wav
cargo run -p chattybara-cli -- modem sweep "hello chattybara" out/modem-sweep
cargo run -p chattybara-cli -- modem samples "hello chattybara" out/modem-samples --overwrite
cargo run -p chattybara-cli -- simulate app-link --payload-bytes 180 --drop-first-attempt --duplicate-deliveries
cargo run -p chattybara-cli -- chat local-peer-script docs/chat/local-peer-basic.txt --station-a JA1TST --station-b JA1QSO --out-dir out/local-peer --overwrite
cargo run -p chattybara-cli -- chat local-peer-script docs/chat/local-peer-app-features.txt --station-a JA1TST --station-b JA1QSO --out-dir out/local-peer-app --overwrite
cargo run -p chattybara-cli -- corpus audit
cargo run -p chattybara-cli -- chat compare-peer-logs docs/chat/basic-observed-log.txt docs/chat/basic-peer-observed-log.txt --station-a JA1TST --station-b JA1QSO
cargo run -p chattybara-cli -- host script out/host-script.txt
```

## Packet Audio Samples

`chattybara modem samples` writes a small RX/TX WAV set for modem backend work:

- `tx-packet.wav`: playback reference for guarded TX tests.
- `rx-clean.wav`: clean receiver golden sample.
- `rx-loopback.wav`: receiver sample with virtual latency and gain.
- `rx-impaired.wav`: receiver sample with deterministic noise and drift.
- `rx-silence.wav`: negative control that must not decode.

Each generated sample has a matching decode JSON where useful, plus
`samples-report.json` for regression checks.

## Live Audio Notes

The live audio path is guarded and dry-run first. Use the virtual lab before
opening host audio devices. When moving to hardware, use receive-only tests,
dummy-load tests, and explicit operator checklists.

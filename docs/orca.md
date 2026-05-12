# orca

orca is the standalone modem engine used by chattybara's current native
backends. It lives outside this repository at
`https://gitlab.com/yokij/orca`.

Use orca directly when you want modem, DSP, audio, frame, fixture, corpus,
host, channel-simulation, or backend/protocol lab tools without the chat
application:

```sh
orca modem roundtrip "hello orca"
orca modem encode "hello orca" out/orca-packet.wav
orca modem decode out/orca-packet.wav
orca modem sweep "hello orca" out/orca-sweep --overwrite
orca fixture synth out/tone.wav --kind tone-burst --frequency 1000
orca dsp tone out/tone.wav
orca frames pipeline out/tone.wav
orca simulate channel --gain 0.8 --snr 24 out/tone.wav out/tone-noisy.wav
```

Chattybara depends on orca crates as a backend implementation detail. The
public chat client docs should stay focused on user workflows and generic modem
backend concepts; protocol-specific lab work belongs in orca.

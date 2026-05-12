# Contributing

chattybara is a clean-room, public-domain research project. Contributions must
preserve that boundary.

## License

By contributing, you agree to dedicate your contribution under CC0 1.0
Universal, the same public domain dedication used by the repository.

## Clean-Room Rule

Implementation work may use:

- Public documentation.
- Public, operator-authored observations.
- Synthetic fixtures and locally generated traces.
- Independently reproducible black-box experiments.
- General math, DSP, networking, terminal UI, and Rust programming knowledge.

Implementation work must not use:

- Disassembly of proprietary modem or chat binaries.
- Runtime memory inspection of proprietary implementations.
- Leaked source, private protocol documents, or license-restricted material.
- Tenuous protocol details hidden through obfuscation or renamed to disguise provenance.

Use `docs/provenance/README.md` labels when adding compatibility-sensitive
facts. Facts labeled `tainted-review` or `blocked` must stay out of public code
and public docs.

## Development Checks

Run these before proposing a change:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo run -p chattybara-cli -- corpus audit
cargo run -p chattybara-cli -- simulate app-link --payload-bytes 180 --drop-first-attempt --duplicate-deliveries
```

No default test may require VARA, VarAC, radio hardware, private captures, or a
virtual audio cable.

## Radio Safety

Changes that can open audio devices, serial ports, PTT, external modem software,
or transmit-capable paths must stay dry-run by default. Live operation needs an
explicit operator flag and transmit operation needs an additional explicit
transmit flag.

## Public Claims

Do not describe chattybara as VARA-compatible, VarAC-compatible, or on-air
interoperable unless the claim is backed by public, reproducible observations
and merged release notes. The current public release is a no-hardware research
preview.

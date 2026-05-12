# observations

Observation manifests describe black-box experiments and their artifacts. They
are separate from small checked-in fixtures because they may point to larger
audio files or opt-in local capture outputs.

Validate an observation manifest with:

```sh
cargo run -p chattybara-cli -- corpus observation validate corpus/observations/example-observation.toml
```

Each observation must use a public provenance label and synthetic or consented
payloads. Tainted or blocked observations may be retained outside implementation
paths for legal review, but the validator rejects them.

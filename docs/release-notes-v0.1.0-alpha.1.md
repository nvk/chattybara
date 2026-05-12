# chattybara 0.1.0-alpha.1

chattybara `0.1.0-alpha.1` is a public alpha for no-hardware terminal radio
chat workflows and generic modem backend integration.

This release includes:

- Rust workspace and `chattybara` TUI chat CLI.
- Canonical orca engine source in `https://gitlab.com/yokij/orca`,
  consumed here as pinned git dependencies.
- Synthetic fixture generation, corpus validation, WAV inspection, DSP traces,
  and receive-pipeline reports.
- Orca packet modem integration for lab round trips.
- Generated TX/RX packet-audio sample sets for no-hardware playback and decode checks.
- Local peer, local node, and TUI chat workflows with `CBAPP/1` app envelopes.
- Reliability simulation for ACKs, retries, duplicate detection, fragments,
  file chunks, hashes, and impaired channels.
- Audio device inventory and guarded IC-705 CI-V dry-run/live serial commands.
- Setup, operator, troubleshooting, provenance, virtual lab, and release docs.
- Local no-hardware checks; hosted CI configs are optional mirrors.

This release does not require radio hardware, virtual audio routing, private
traffic, hosted CI, or live serial/audio access for its default checks.

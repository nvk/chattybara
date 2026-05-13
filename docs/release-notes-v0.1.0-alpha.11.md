# chattybara 0.1.0-alpha.11

chattybara `0.1.0-alpha.11` adds a no-hardware protocol scaffold suite for the
next non-orca modes.

## Changes

- Added `chattybara station protocol-suite` for JS8Call, WSJT-X/FT8, fldigi,
  CW assist, PSK Reporter, Winlink-VARA, and Winlink-orca fixture validation.
- The suite can write replayable JSONL fixtures plus `support.json` metadata
  under `--out-dir`.
- External adapter reports now include protocol metadata and safer endpoint
  defaults. JS8Call now defaults to `127.0.0.1:2442`.

## Test

```sh
chattybara station protocol-suite --station JA1TST --out-dir out/protocol-suite
chattybara station replay out/protocol-suite/pskreporter/events.jsonl
```

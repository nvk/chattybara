# Distribution

The clean public topology is:

- `chattybara`: public chat client at `https://github.com/nvk/chattybara`.
- `orca`: public modem engine at `https://gitlab.com/yokij/orca`.

Chattybara can be installed from source today. Binary archives and a Homebrew
tap can be added after the new public repositories are created and tagged.

## Source Install

```sh
cargo install --git https://github.com/nvk/chattybara.git --tag v0.1.0-alpha.7 --locked --package chattybara-cli --bin chattybara
```

During local development:

```sh
cargo install --path crates/chattybara-cli --locked --force
chattybara --help
chattybara chat tui
```

## Build Binary Archives

```sh
CARGO_HOME=$PWD/.cargo-home scripts/build-release-asset.sh 0.1.0-alpha.7
```

The script writes:

- `dist/chattybara-0.1.0-alpha.7-aarch64-apple-darwin.tar.gz`
- `dist/chattybara-0.1.0-alpha.7-aarch64-apple-darwin.tar.gz.sha256`

Build orca release assets from `https://gitlab.com/yokij/orca`, not from
this chat client repo.

## Homebrew Tap Direction

The chattybara tap should be created after the new public repositories exist. A
conservative layout is:

```text
Formula/chattybara.rb
README.md
```

The `chattybara` formula should point at GitHub release assets for the chat
client. Orca should be distributed from Yoki's separate
`https://gitlab.com/yokij/homebrew-orca` tap.

## Binary Smoke Test

After publishing binary assets and a tap:

```sh
brew uninstall chattybara || true
brew install chattybara
chattybara --help
chattybara chat tui --setup-preview
chattybara modem roundtrip "hello chattybara"
chattybara simulate app-link --payload-bytes 180 --drop-first-attempt --duplicate-deliveries
chattybara station config --station JA1TST
chattybara winlink telnet --check
chattybara winlink sync --transport telnet
```

Run orca binary checks in the orca repository.

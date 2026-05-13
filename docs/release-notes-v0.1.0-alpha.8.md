# chattybara 0.1.0-alpha.8

chattybara `0.1.0-alpha.8` adds macOS Keychain-backed Winlink password
storage, so live Telnet/CMS sync no longer requires exporting the Winlink
password into every shell session.

## Changes

- Added `chattybara winlink account password set` for secure interactive
  password entry.
- Added `chattybara winlink account password set --password-stdin` for
  scripted setup without putting the secret in process arguments.
- Added `chattybara winlink account password status` and `delete`.
- Live Telnet/CMS sync now reads a configured `keychain` password source.
- `CHATTYBARA_WINLINK_PASSWORD` remains supported as a temporary override.

## Basic Flow

```sh
chattybara winlink account password set
chattybara winlink sync --transport telnet --live --allow-send
```

For scripted setup:

```sh
printf '%s\n' "$WINLINK_PASSWORD" | chattybara winlink account password set --password-stdin
```

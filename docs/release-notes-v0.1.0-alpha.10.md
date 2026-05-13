# chattybara 0.1.0-alpha.10

chattybara `0.1.0-alpha.10` fixes Winlink station selection for live sync.

## Changes

- `chattybara winlink ...` commands now auto-select the single configured
  Winlink account store when `--station` is omitted.
- This prevents live Telnet/CMS sync from falling back to the sample station
  while the real Winlink account and Keychain password live in a different
  store.

## Test

```sh
chattybara winlink account status
chattybara winlink sync --transport telnet --live --allow-send
```

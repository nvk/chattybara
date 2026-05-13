# chattybara 0.1.0-alpha.9

chattybara `0.1.0-alpha.9` fixes the live Telnet/CMS B2F handshake used by
Winlink sync.

## Changes

- Send the local forwarder line as `;FW: CALL`, matching the B2F reference
  handshake.
- Send the local prompt/comment line before beginning B2F traffic.
- Tighten fake-CMS tests so the transcript regression catches this class of
  live protocol bug.

## Test

```sh
chattybara winlink sync --transport telnet --live
```

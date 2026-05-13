# chattybara 0.1.0-alpha.12

chattybara `0.1.0-alpha.12` turns the external protocol surfaces from static
scaffolds into live-capable, no-hardware-testable adapters.

## Highlights

- `station external --adapter js8call --live` can connect to the JS8Call TCP
  JSON API, read directed messages/spots/activity, and send only behind
  `--enable-tx --allow-transmit`.
- `station external --adapter wsjtx --live` can listen for WSJT-X/FT8 UDP
  decode datagrams. Fixture datagrams can also be replayed from disk.
- `station external --adapter fldigi --live` can read fldigi RX text through
  XML-RPC and can add/transmit text only behind the explicit TX gate.
- `station external --adapter pskreporter --live` can query PSK Reporter and
  convert reception reports into station spot events.
- `station external --adapter cw-assist --fixture FILE` decodes receive-only
  plain-text or dot/dash Morse fixtures.
- `winlink transport --transport vara --live` now probes the external VARA
  command port for status only. It does not key PTT, open a data session, or
  sync mail.

## Quick Checks

```sh
chattybara station external --adapter js8call
chattybara station external --adapter cw-assist --fixture cw.txt --out out/cw.jsonl
chattybara winlink transport --transport vara
```

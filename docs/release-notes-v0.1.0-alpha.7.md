# chattybara 0.1.0-alpha.7

chattybara `0.1.0-alpha.7` adds full live Telnet/CMS B2F message receive for
supported proposals and guarded live outbox sending.

## Highlights

- Live sync accepts supported inbound `FC` and `FD` proposals instead of
  deferring all payloads.
- B2F binary block checksums, B2 LZHUF CRCs, LZHUF/gzip decompression, message
  headers, bodies, and attachments are handled.
- Received attachments are saved beside the local Winlink store under
  `attachments/<message-id>/`.
- Existing metadata placeholders are replaced with downloaded messages when
  the same message ID is seen again.
- Queued outbox messages can be sent over Telnet/CMS only with explicit
  `--allow-send`.

## Try It

```sh
chattybara station config --station JA1TST
chattybara winlink sync --transport telnet --live
chattybara winlink inbox
chattybara winlink read MESSAGE-ID
```

Live sending remains operator-gated:

```sh
chattybara winlink compose --to JA1QSO --subject "test" --body "hello"
chattybara winlink sync --transport telnet --live --allow-send
```

VARA and orca Winlink transports remain planned; this release improves the
Telnet/CMS path.

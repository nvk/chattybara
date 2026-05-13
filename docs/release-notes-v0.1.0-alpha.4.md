# chattybara 0.1.0-alpha.4

chattybara `0.1.0-alpha.4` adds the first live Winlink inbox check.

## Highlights

- Live Telnet/CMS sync can authenticate and list pending inbox message
  metadata.
- Message payloads are explicitly deferred, so bodies and attachments are not
  downloaded or cleared in this build.
- Local station settings keep operator call signs outside committed source and
  examples.
- Winlink passwords stay out of command-line arguments. Use
  `CHATTYBARA_WINLINK_PASSWORD` for live Telnet/CMS sync.
- New no-network regression coverage simulates CMS login, secure challenge
  response, B2F proposal checksums, and metadata storage.

## Try It

```sh
export CHATTYBARA_WINLINK_PASSWORD='your-winlink-password'
chattybara station config --station JA1TST
chattybara winlink sync --transport telnet --live
chattybara winlink inbox
unset CHATTYBARA_WINLINK_PASSWORD
```

This is still an alpha. Full Winlink message body download, attachment
download, live sending, VARA, and orca Winlink transports remain guarded.

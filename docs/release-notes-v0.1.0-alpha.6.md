# chattybara 0.1.0-alpha.6

chattybara `0.1.0-alpha.6` fixes live Winlink Telnet/CMS receive sync.

## Highlights

- The live sync path now consumes full CR-terminated `Callsign :` and
  `Password :` prompts before sending responses.
- Access-code and B2F command lines are sent as complete CR-terminated writes.
- The default endpoint is now `cms-z.winlink.org:8772`, which accepts unknown
  alpha client identifiers that production CMS rejects.
- The receive sequence now sends `;FW`, the local SID, and `FF` before parsing
  pending inbound proposal metadata.

## Try It

```sh
chattybara station config --station JA1TST
chattybara winlink sync --transport telnet --live
chattybara winlink inbox
```

Live sync is still receive-only and metadata-only. Message bodies and
attachments remain deferred.

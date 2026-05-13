# chattybara 0.1.0-alpha.5

chattybara `0.1.0-alpha.5` fixes the live Winlink Telnet/CMS login preamble.

## Highlights

- Default CMS endpoint is now `cms.winlink.org:8772`.
- The Telnet access code is sent as `CMSTELNET`.
- The Telnet login prompt reader now consumes the full `Callsign :` and
  `Password :` prompts before sending responses.

## Try It

```sh
chattybara station config --station JA1TST
export CHATTYBARA_WINLINK_PASSWORD='your-winlink-password'
chattybara winlink sync --transport telnet --live
chattybara winlink inbox
unset CHATTYBARA_WINLINK_PASSWORD
```

Live sync is still receive-only and metadata-only. Message bodies and
attachments remain deferred.

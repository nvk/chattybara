# Winlink

chattybara includes an early transport-neutral Winlink mailbox stack. The
current release is meant for no-radio testing and operator workflow build-out.

## What Works Now

- Local Winlink account/store setup.
- Local message compose/read/list workflows.
- Deterministic fake sync that receives one fixture inbox message and moves
  queued outbox messages to sent.
- B2F proposal modeling for queued messages.
- Telnet/CMS dry-run status checks.
- Optional live Telnet/CMS TCP connectivity check that does not send a
  password or message payload.
- VARA and orca transport status surfaces, guarded as planned transports.

## What Is Guarded

Full live Winlink message exchange is not enabled yet. `--live` is only useful
for a Telnet/CMS TCP connectivity check. VARA and orca live sync report planned
transport status until their session layers are implemented.

Do not pass a Winlink password on the command line. The account profile records
only the intended credential source:

- `none`
- `env`
- `keychain`

## Setup

Use a test store while experimenting:

```sh
chattybara winlink account setup \
  --station VE3TST \
  --store out/winlink/store.json \
  --password-source env
```

Without `--store`, chattybara uses:

```text
~/.local/share/chattybara/winlink/CALL/store.json
```

## Compose And Fake Sync

```sh
chattybara winlink compose \
  --station VE3TST \
  --store out/winlink/store.json \
  --to JA1QSO \
  --subject "no radio test" \
  --body "testing chattybara winlink fake sync"

chattybara winlink outbox --station VE3TST --store out/winlink/store.json
chattybara winlink sync --station VE3TST --store out/winlink/store.json --transport fake
chattybara winlink inbox --station VE3TST --store out/winlink/store.json
chattybara winlink read FAKE-VE3TST-001 --station VE3TST --store out/winlink/store.json
```

## Telnet/CMS Connectivity

Dry-run check:

```sh
chattybara winlink telnet --station VE3TST --check
```

Live TCP check only:

```sh
chattybara winlink telnet --station VE3TST --check --live
```

The live check opens TCP to the configured CMS endpoint and reads an initial
greeting if one is available. It does not authenticate, upload, download, or
send a password.

## VARA And Orca Transport Plan

VARA is modeled as an external operator-installed modem transport:

```sh
chattybara winlink transport --station VE3TST --transport vara
```

orca is modeled as the experimental open modem transport:

```sh
chattybara winlink transport --station VE3TST --transport orca
```

The mailbox store, outbox, attachments, B2F message model, and safety gates live
in chattybara. Packet, audio, link, and modem mechanics belong in orca.

## Release Gates For Full Live Sync

Before full live Winlink sync is enabled:

- fake sync must stay deterministic and covered by tests.
- B2F parser/serializer coverage must include message proposals, payload
  checksums, compression, attachments, accepts/rejects, aborts, and reconnects.
- credentials must use keychain/env sources, never command-line flags or logs.
- live send must require explicit `--live --allow-send`.
- Telnet/CMS, VARA, and orca transports must share the same store and reporting
  model.

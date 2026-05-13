# chattybara 0.1.0-alpha.3

chattybara `0.1.0-alpha.3` adds the first Winlink mailbox workflow.

Highlights:

- New `chattybara-winlink` crate for transport-neutral Winlink mailbox state.
- Local Winlink account/store setup, compose, inbox, outbox, read, and fake
  sync commands.
- B2F message proposal reporting for queued mail.
- Telnet/CMS dry-run and optional live TCP connectivity check that does not
  send credentials or message payloads.
- Guarded VARA and orca transport status surfaces for future Winlink-over-VARA
  and Winlink-over-orca work.
- Winlink mode registry entries and `/workspace winlink` TUI workspace
  selection.

Full live Winlink message exchange is still guarded. Use fake sync for
no-radio workflow testing. Live Telnet/CMS currently checks TCP connectivity
only.

# roadmap

This roadmap is intentionally conservative. The current release is a public
alpha for no-hardware chat-client development, packaging, and generic modem
backend work.

## 0.1.x public alpha hardening

- Keep `chattybara chat tui` as the primary user entry point.
- Move setup, station identity, backend selection, audio devices, radio profile
  selection, loopback tests, and config save/load into the TUI.
- Improve pane navigation, selectable mailbox and file offers, composer modes,
  backend/radio/audio status, and error states.
- Keep `chattybara` consuming `orca-*` as explicit git dependencies for
  alpha, then move to crates.io once APIs stabilize.
- Package chattybara from `https://github.com/nvk/chattybara`.
- Keep orca packaged separately from `https://gitlab.com/yokij/orca`.
- Add Linux and Intel macOS binaries when a trustworthy local or hosted release
  path is available.

## 0.2.x live station setup

- Expand the TUI from one chat workspace into explicit chat, weak-signal,
  CW-assist, spot-monitor, operator-console, and rig/setup workspaces.
- Harden live audio device selection and saved audio profiles.
- Add guided IC-705 and generic Hamlib setup flows in the TUI.
- Keep all transmit-capable flows opt-in, visibly marked, and dry-run by
  default.
- Add operator checklists for receive-only testing, dummy-load testing, and
  controlled local audio loopback.
- Expand no-hardware simulators for dropped packets, duplicate packets, bad
  SNR, drift, latency, and partial file-transfer recovery.

## 0.3.x backend expansion

- Promote receive-only external adapter scaffolds into fixture-backed adapters
  for JS8Call, WSJT-X, fldigi, and PSK Reporter before adding any TX/reporting
  paths.
- Continue Winlink mailbox/workflow backend work after the initial local store,
  fake sync, B2F proposal helper, and Telnet/CMS dry-run surfaces.
- Keep the Winlink mail model transport-neutral so the same inbox, outbox,
  drafts, attachments, receipts, and sync status can run over multiple links.
- Add Winlink-over-VARA as an external modem adapter after Telnet/CMS is
  stable. Treat VARA as an operator-installed external modem/backend, guarded
  by explicit live/TX opt-ins and fixture-backed tests.
- Add Winlink-over-orca as the experimental open modem path. Orca should own
  the packet/audio/link mechanics; chattybara should own the Winlink mailbox,
  session UI, safety gates, and operator workflow.
- Add backend adapters for documented modem/protocol stacks where possible.
- Keep protocol-specific compatibility claims out of chattybara unless they are
  backed by backend-specific documentation and tests.
- Treat orca on-air modem compatibility as a separate milestone from
  chattybara's user experience.

## not planned for alpha

- No default test that requires radio hardware, virtual audio cable, private
  traffic, hosted CI, or live serial/audio access.
- No crates.io publication until the public API is intentionally stabilized.

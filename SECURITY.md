# Security

chattybara is a research preview for digital radio modem and chat experiments.
Security reports can include conventional software issues, unsafe hardware
control behavior, privacy leaks in logs or captures, and misleading protocol
compatibility claims.

## Reporting

Use a confidential GitLab issue when the report contains sensitive details. Use
a normal GitLab issue for non-sensitive defects.

Do not attach private radio traffic, private call logs, proprietary binaries, or
license-restricted protocol material to public issues. Use synthetic fixtures or
operator-authored minimal reproductions whenever possible.

## Supported Version

Only the current `master` branch and the latest public release tag are in scope.

## Safety Expectations

Reports that involve radio hardware should include:

- The exact command that was run.
- Whether `--live`, `--allow-transmit`, or `CHATTYBARA_EXTERNAL_MODEM=1` was used.
- The audio and serial devices selected.
- Whether the station was connected to a dummy load, non-radiating path, or antenna.

chattybara defaults to no-hardware and dry-run behavior. Any path that violates
that expectation is release-blocking.

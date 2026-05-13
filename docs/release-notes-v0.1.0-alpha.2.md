# chattybara 0.1.0-alpha.2

chattybara `0.1.0-alpha.2` is a small TUI usability fix release.

This release fixes the setup status bar so `/station CALL` immediately updates
the visible station identity while setup is still open. Previously the setup
pane changed, but the top status bar continued to show the active backend's old
station until `/start`.

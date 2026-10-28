# Release validation

Run automated tests first. Record manual results rather than assuming that an SSH protocol test verifies keyboard behavior.

## Recorded results — 2026-10-29

Version 0.2.1:

- 13 unit tests passed on Windows x64.
- Windows and Linux x86_64 static analysis passed with warnings treated as errors.
- Six live tests passed over direct SSH to Linux x86_64: application input receiving a path, shell and tmux command preservation without automatic Enter, tmux paste, target-change protection, and verified uploads/cancellation/corrupt-data rejection.
- The same six live tests passed through OpenSSH ProxyJump, using the test host as a bastion to its own loopback SSH endpoint. Two independent servers were not exercised.
- Linux arm64 helper is cross-compiled; execution on arm64 hardware has not been verified.
- The packaged Windows executable and embedded x86_64 helper passed version/setup diagnostics on the test host.

The user manually confirmed screenshot upload and Ctrl+V path insertion in version 0.1.0. The version 0.2.1 dismissal of the progress popup still needs a manual Windows Terminal check. Multi-window focus, password/MFA authentication, and the remaining cases below are not marked as verified.

## Manual checklist

- [ ] Copy a screenshot with transparency; paste via Ctrl+V at a shell prompt. Confirm a readable remote PNG and its quoted path at the cursor, without an automatic Enter or success popup.
- [ ] Copy two Explorer files with the same basename from different directories. Confirm both remote hashes and distinct paths.
- [ ] Type `file `, then paste using Ctrl+Shift+V. Confirm a quoted path is inserted without pressing Enter.
- [ ] Paste into a terminal application's input prompt with Ctrl+V. Confirm the saved path is inserted without pressing Enter.
- [ ] Run `clippy last` locally. Confirm the receipt identifies the server and saved paths.
- [ ] Paste Unicode/multiline text. Confirm Windows Terminal's existing warnings and text behavior.
- [ ] Open two Clippy tabs and split panes connected to different hosts; confirm each paste reaches only the focused session.
- [ ] Switch focus during upload. Confirm no insertion into another process or tmux pane.
- [ ] Open Windows Terminal settings/command palette and another app. Confirm Clippy does not intercept their paste.
- [ ] Cancel a transfer with Ctrl+C. Confirm no path is pasted, staging files disappear, and the shell remains usable.
- [ ] Test an ordinary SSH tab alongside Clippy; its shortcuts must remain native.
- [ ] Test tmux attach/detach and split panes, including another attached client changing the active pane.
- [ ] Test a connection through ProxyJump.
- [ ] Test password/passphrase/MFA authentication using OpenSSH's own prompt.
- [ ] Test connection loss, unwritable upload directory, and helper version mismatch.

Use disposable sessions and generated test files. Do not modify unrelated services or existing tmux sessions.

# Release validation

Run automated tests first. Record manual results rather than assuming that an SSH protocol test verifies keyboard behavior.

## Recorded results — 2026-10-29

Version 0.3.1 adds a single-file Windows installer and a gated release workflow. Local Rust tests and static analysis pass. The release workflow must pass the fresh-install/repeat-install/rollback/uninstall tests before publishing assets; its run is linked from the release commit's GitHub Actions history. The package is unsigned.

A production client receipt after the final 0.3.0 pipe fix records a successfully inserted file path under the switched Linux user's private home. This confirms that transfer/path insertion occurred in the installed client. It does not by itself verify every mouse/UI behavior in the manual checklist.

Version 0.3.0 development checks:

- 23 Windows unit tests passed, including profile encoding/preservation, native-routing decisions, stale/ambiguous focus rejection, session-registration cleanup, queued paste/cancel delivery, local framing bounds, and disabling inherited console mouse reporting.
- Windows and Linux static analysis passed before the final packaging pass.
- Background start, local session registration/unregistration, pause, resume, status, and stop passed on Windows.
- Native `ssh -V` passthrough passed.
- The isolated background runtime test passed with captured output: startup returned without retaining the caller's output handles, session registration/unregistration and pause/resume worked, and an unrelated process was rejected by the IPC server.
- Six existing direct SSH transfer tests passed with the 0.3 helper.
- Two additional live sudo/su tests passed: upload/read as a different UID, mode 0600, command preservation without Enter, cancellation and corrupt-data cleanup, return to the original user, and switched-shell exit preventing insertion into the parent.
- All eight live tests also passed through ProxyJump, using the test host as a bastion to its own loopback SSH endpoint.
- User installation, repeat-install idempotency, startup registry entry, new PowerShell profile loading, and native SSH remote-command passthrough passed. Existing sessions were retained.
- The first manual 0.3 paste attempt failed. Inspection found concurrent reads/writes on cloned synchronous named-pipe handles could block focus updates. The bridge now uses alternating poll/reply exchanges, and the isolated Windows runtime check passes 100 consecutive focus exchanges before unregistering. Manual verification of that correction is pending.

Startup-at-sign-in has not been verified by logging the user out. The mouse reporting correction and final Ctrl+V behavior still require the manual checks below.

Version 0.2.1:

- 13 unit tests passed on Windows x64.
- Windows and Linux x86_64 static analysis passed with warnings treated as errors.
- Six live tests passed over direct SSH to Linux x86_64: application input receiving a path, shell and tmux command preservation without automatic Enter, tmux paste, target-change protection, and verified uploads/cancellation/corrupt-data rejection.
- The same six live tests passed through OpenSSH ProxyJump, using the test host as a bastion to its own loopback SSH endpoint. Two independent servers were not exercised.
- Linux arm64 helper is cross-compiled; execution on arm64 hardware has not been verified.
- The packaged Windows executable and embedded x86_64 helper passed version/setup diagnostics on the test host.

The user manually confirmed screenshot upload and Ctrl+V path insertion in version 0.1.0. The version 0.2.1 dismissal of the progress popup still needs a manual Windows Terminal check. Multi-window focus, password/MFA authentication, and the remaining cases below are not marked as verified.

## Manual checklist

- [ ] Install once, open a new PowerShell tab, and connect with ordinary `ssh`. Confirm image/file paste without a Clippy launcher command.
- [ ] Pause/resume from the tray; confirm native text paste remains available.
- [ ] Stop and restart the background app while SSH stays open. Confirm it re-registers and file paste resumes.
- [ ] Verify sign-in startup and uninstall on a disposable Windows profile.

- [ ] Copy a screenshot with transparency; paste via Ctrl+V at a shell prompt. Confirm a readable remote PNG and its quoted path at the cursor, without an automatic Enter or success popup.
- [ ] Move the mouse over an ordinary remote prompt and after `sudo su USER`; no mouse-report numbers should appear. Check mouse support in a remote TUI that explicitly enables it.
- [ ] After `sudo su USER`, paste a screenshot and verify the switched user can read the inserted path.
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

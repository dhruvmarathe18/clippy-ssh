# Clippy SSH

[![Checks](https://github.com/dhruvmarathe18/clippy-ssh/actions/workflows/check.yml/badge.svg)](https://github.com/dhruvmarathe18/clippy-ssh/actions/workflows/check.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

**Copy locally. Paste into your SSH session.**

Copy a screenshot or files on Windows. Press **Ctrl+V** in an SSH session. Clippy saves the files on the server and inserts their remote paths at the cursor, without a success popup or automatic Enter.

Clippy is a Rust client and Linux helper. It uses your installed Windows OpenSSH client and Windows Terminal. Ordinary uploads require no cloud service, remote listening port, administrator access, or password storage. Pasting after switching Linux users uses existing sudo permissions, as described below.

## Status and direction

This is an early working prototype, version **0.3.0**. A background Windows app handles paste shortcuts, and one-time PowerShell integration enables clipboard transfer when you use your normal `ssh` command in Windows Terminal. Normal text paste keeps the terminal's existing behavior.

The goal is to make clipboard transfer feel native across terminal clients. Windows Terminal with PowerShell is the first integration; VS Code, WSL, PuTTY, and other clients remain planned. See the [roadmap](docs/roadmap.md) and [contribution guide](CONTRIBUTING.md) to help build them.

### Example

Type `file ` at the remote prompt, copy a screenshot on your laptop, then press Ctrl+V:

```text
user@server:~$ file '/home/user/.local/share/clippy/uploads/<batch-id>/0/clipboard.png'
```

The image is already on the server. The command remains editable until you press Enter. A terminal application can receive the uploaded path in the same way.

## Use

Build the [Windows package](#build), extract `dist/clippy-windows-x64.zip`, and run once from the extracted folder:

```powershell
.\clippy.exe install
```

Open a **new PowerShell tab in Windows Terminal**, then connect normally:

```powershell
ssh -i "$HOME\.ssh\my-key" user@server
```

Clippy starts at sign-in and is also started automatically when needed by a new supported session. The Linux helper is installed in the remote user's home on first use of each version. Automatic setup uses additional SSH connections, so password/passphrase/MFA configurations may prompt more than once. Host-key checking remains with OpenSSH.

Existing SSH aliases and jump hosts work:

```powershell
ssh -J bastion destination
```

1. Copy a screenshot, a browser image, or one or more files in Explorer.
2. Focus the Clippy terminal pane and press **Ctrl+V**.
3. A small status popup shows progress. Terminal input pauses during the upload; **Ctrl+C** cancels it. Typing during the pause is discarded.
4. After the whole batch is verified, the remote paths appear at the cursor and the progress popup disappears. This works at a shell prompt and inside terminal applications. There is no success confirmation, and Clippy never presses Enter.

**Ctrl+Shift+V** does the same thing. For example, type `file ` and then press Ctrl+V to insert an uploaded image's path. Run `clippy last` in a local PowerShell tab to see the latest saved paths.

Text uses Windows Terminal's existing paste behavior, including its multiline warnings. The clipboard is not replaced with paths. Other panes and apps keep their existing behavior. Right-click and Shift+Insert use the terminal's existing paste behavior and do not trigger Clippy uploads.

**A file path is not a shell command.** Pressing Enter on a bare PNG path asks Bash to execute the image and produces `Permission denied`. Use the inserted path as an argument, such as `file <pasted-path>`, or paste it into an AI tool's prompt.

Uploaded paths are POSIX-shell-quoted and separated by spaces. In an AI tool, ask it to read the supplied paths; Clippy does not create a tool-specific attachment widget. Windows-side WSL sessions are not supported in this version: launch the Windows executable from PowerShell.

## Background controls and native SSH

The tray icon has pause/resume and exit controls. The same controls are available locally:

```powershell
clippy background status
clippy background pause
clippy background resume
clippy background stop
clippy background start
```

Stopping the background app disables rich paste without terminating SSH connections. Registered clients reconnect when the background app starts again. There is no continuous clipboard capture.

The PowerShell integration handles supported interactive shell connections only. Remote commands, tunnels, redirected input/output, unsupported options, and unsupported effective SSH configuration go through native OpenSSH. Other terminal clients also stay native. Use `ssh.exe ...` explicitly, or set `$env:CLIPPY_DISABLE = '1'`, to bypass integration. Clear that variable to re-enable it.

Install adds marked blocks to the current user's Windows PowerShell and PowerShell all-host profiles, a generated script under `%LOCALAPPDATA%\Clippy`, and an `HKCU\Software\Microsoft\Windows\CurrentVersion\Run\ClippySSH` startup value. It backs up existing profiles before changes and preserves existing functions/aliases named `ssh` or `clippy`. It does not change PowerShell execution policy. Profiles must be allowed by your existing policy.

Run `clippy uninstall` to remove the managed profile blocks and startup entry. Open new tabs to reload profiles. Existing SSH sessions, binary versions, and uploaded files are retained. Installation and upgrades do not replace running executables.

For explicit use without shell/startup integration, keep both executables together and run `clippy setup ...` followed by `clippy ssh ...`. Already-running native SSH sessions cannot acquire file paste retroactively.

## Files and retention

- Clipboard bitmaps become PNGs; copied files retain their original bytes.
- Explorer file lists take priority over image representations; images take priority over text.
- Uploads are saved under `~/.local/share/clippy/uploads/<batch-id>/<item-number>/<filename>` with private permissions.
- Same-named files are kept separately. No existing destination file is overwritten.
- Staging data is discarded when a transfer fails or is cancelled. Files are finalized only after size and SHA-256 verification.
- Completed uploads persist across SSH and tmux sessions.
- If the destination process or tmux pane changes mid-upload, files are retained and their paths are not injected into another prompt. `clippy last` displays the latest completed upload receipt on the laptop.

Explicit cleanup only removes old, manifest-bearing Clippy batches:

```powershell
.\clippy.exe clean --older-than 30d user@server
```

## Supported environments

Windows x64, Windows Terminal 1.24+, Windows OpenSSH, and Linux x86_64/arm64. The remote account needs a writable home, PTY support, `/proc`, and `sha256sum` for helper setup. Helpers are statically linked with musl.

Remote Bash/Zsh shells and tmux on the default socket are supported. After `sudo su other-user`, Clippy follows the foreground process chain and saves private files in that user's home, owned by that user. This requires the original login account to have noninteractive sudo permission to run the installed helper as root; the upload receiver drops to the destination UID and groups before reading file-transfer messages. It never changes sudoers, requests a password, or makes uploads world-readable. If permission or destination detection fails, paste reports an error.

Nested SSH, containers with a different filesystem, tmux launched after switching users, custom tmux sockets, Windows/macOS servers, directories, virtual attachments such as Outlook objects, and remote-to-local file copying are outside this version. Clipboard PNG data is limited to 256 MiB; ordinary files are streamed with bounded memory. Upload resume is not supported.

## Diagnostics

```powershell
.\clippy.exe doctor
.\clippy.exe last
```

If registration fails, focus the Windows Terminal pane and allow application title changes for that profile, then reconnect. Clippy temporarily marks the pane title to register its accessibility element and restores the title. It does not guess a destination when registration or focus verification fails. Elevated terminal sessions should be launched consistently at the same integrity level.

The background app owns one paste hook per installation/logon. Sessions register through a local named pipe restricted to the current logon; remote pipe clients are rejected, and both endpoints check the other process's executable path. Clipboard payloads and SSH credentials do not travel over this local control channel. A client rechecks its focused pane before reading the clipboard. Input, clipboard contents, and SSH credentials are not logged. The optional local receipt contains the destination, remote paths, and completion result.

`clippy background check` verifies local session registration and cleanup without reading clipboard contents or typing into a terminal.

## Build

The recommended Windows setup is stable Rust with the MSVC toolchain and the Visual C++ build tools. Clone this repository, then run:

```powershell
git clone https://github.com/dhruvmarathe18/clippy-ssh.git
cd clippy-ssh
cargo test --locked
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
.\scripts\package.ps1
```

`package.ps1` builds both static Linux helpers, embeds them in the Windows client, builds the background app, and creates `dist/clippy-windows-x64.zip`. Keep `clippy.exe` and `clippy-background.exe` together. A development build without embedded helpers can install one using `clippy setup --agent PATH [SSH OPTIONS] HOST`.

Windows GNU builds use LLVM-MinGW (MSVCRT), including `ld.lld.exe` and `dlltool` on PATH. The GNU configuration links directly with Rust's bundled CRT to avoid mixing runtime startup objects. `scripts/env.ps1` supports the maintainer's optional workspace-local `.tools` layout; it is not required for a normal Rust installation. See [CONTRIBUTING.md](CONTRIBUTING.md) for platform-specific development notes.

## Tests

Unit tests exercise framing bounds, fragmented terminal sequences, path quoting, filename validation, SHA-256 integrity, atomic batch finalization, cancellation cleanup, empty files, name collisions, and SSH argument validation.

Live tests are opt-in. Set a JSON array of SSH options and the host after installing the matching helper:

```powershell
$env:CLIPPY_TEST_SSH_ARGS = '["-i","C:\\path\\to\\test-key","user@test-host"]'
cargo test --test ssh_live -- --nocapture --test-threads=1
```

These tests create uniquely named Clippy fixtures and an isolated tmux server. Cleanup removes only their own fixtures and uploaded batches. They never use an existing tmux server.

Set `CLIPPY_TEST_SWITCH_USER` to an existing disposable Linux username to also test `sudo su`, private ownership, cancellation, corrupt-data rejection, returning to the original user, and a user-switch destination exiting during upload. The login account must already have the sudo permissions described above; tests do not create users or edit permissions.

Before relying on a release, manually check normal Ctrl+V with a screenshot and Explorer files in multiple Windows Terminal tabs/panes, text paste in an ordinary SSH tab, cancellation, and tmux. See [validation checklist](docs/validation.md).

## Implementation

`clippy` runs `ssh -T` with the installed helper as its remote command. The helper creates a Linux PTY for your login shell. A bounded binary protocol multiplexes terminal bytes, resizing, and verified uploads over that connection. File bytes are never pasted into the shell or encoded as shell commands.

The Windows bridge verifies the actual focused UI Automation terminal element before accepting rich paste. The helper snapshots the destination foreground process and its start time, and the exact tmux pane when applicable. These are checked again before path insertion. Tmux insertion uses an explicitly targeted, temporary paste buffer with bracketed paste enabled when requested by the application.

## Contributing

Issues and pull requests are welcome. Useful contributions include testing different Windows Terminal layouts, improving installation and session routing, and adding terminal adapters. Start with [CONTRIBUTING.md](CONTRIBUTING.md), the [architecture overview](docs/architecture.md), and the [roadmap](docs/roadmap.md).

Report vulnerabilities privately using the instructions in [SECURITY.md](SECURITY.md).

## License

[MIT](LICENSE). This project is independent of the Rust Clippy linter and of OpenSSH.

# Roadmap

The long-term goal is simple: keep Clippy running on the laptop, connect normally, and paste local files or screenshots into a remote terminal without thinking about transfer commands.

These are directions for contribution, not claims that every SSH client already works or promises of delivery dates.

## Working today

- Rust Windows client and Linux helper, connected through native OpenSSH.
- Clipboard images and Explorer files uploaded with size and SHA-256 verification.
- Remote paths inserted at the cursor without executing a command or showing a success popup.
- Ordinary text paste, private persistent uploads, cancellation, and explicit cleanup.
- Direct SSH, ProxyJump, and targeted paste in supported tmux sessions.
- Uploads owned by the foreground Linux user after sudo/su, with existing noninteractive sudo permission.
- A Windows background app with a tray menu, pause/resume, and sign-in startup.
- Logon-restricted local session registration, peer process checks, and focus heartbeats.
- Reversible PowerShell integration for supported interactive `ssh` connections in Windows Terminal, with native passthrough for other invocations.
- A single per-user Windows EXE installer with startup, uninstall, configuration rollback, and bundled helpers.

## Automatic connections: expand coverage

Version 0.3 implements the first integration with `clippy install`. Extend the supported interactive SSH options and test more authentication/configuration combinations while keeping native passthrough for unsupported cases. Profiles can be uninstalled, and `ssh.exe` remains a bypass.

This needs careful argument handling and tests for aliases, identity files, jump hosts, authentication prompts, and SSH used by other programs. The current wrapper accepts only a subset of interactive SSH options; it must not silently replace all `ssh` invocations.

## Background Windows app: hardening

Version 0.3 separates the global paste hook into `clippy-background.exe`, with a session registry and a local control channel. File transfer and remote PTY handling remain in each client. Expand manual coverage and improve recovery and diagnostics before calling the experience stable.

Work to validate and improve:

- Broader routing tests across windows, tabs, panes, elevated sessions, and process restarts.
- Additional malformed-message and disconnect tests for local IPC.
- Startup, pause, and uninstall behavior across Windows configurations and redirected Documents folders.
- Installer rollback and upgrade ergonomics when sessions from older installations are still active.

An always-running app by itself does not identify the remote destination of every terminal. The session integration still needs to establish where a paste belongs and how the file will be transferred.

## More terminal clients and environments

Explore an adapter interface for Windows Terminal, VS Code, WSL, and other SSH clients. Each adapter should explain how it identifies the focused session, authenticates the transfer, and inserts a path without running it. Add support only after those behaviors can be tested.

Nested SSH, containers, tmux after a user switch, custom tmux sockets, and additional remote operating systems need explicit destination handling. They should not inherit an outer session's upload location by assumption. The current sudo/su integration requires existing noninteractive sudo permission; broader credential/session integration remains future work.

## Reliability and distribution

- More installer rollback/upgrade coverage and fewer authentication round trips for remote helper setup.
- Code signing and broader installer compatibility testing.
- Broader manual compatibility results and live arm64 testing.
- Recovery from interrupted transfers and clearer failure diagnostics.
- Directory transfer and virtual clipboard formats after their semantics are defined.

Propose a focused piece of this work in an issue, with the environment you can test. See [CONTRIBUTING.md](../CONTRIBUTING.md) for development instructions.

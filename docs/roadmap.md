# Roadmap

The long-term goal is simple: keep Clippy running on the laptop, connect normally, and paste local files or screenshots into a remote terminal without thinking about transfer commands.

These are directions for contribution, not claims that every SSH client already works or promises of delivery dates.

## Working today

- Rust Windows client and Linux helper, connected through native OpenSSH.
- Clipboard images and Explorer files uploaded with size and SHA-256 verification.
- Remote paths inserted at the cursor without executing a command or showing a success popup.
- Ordinary text paste, private persistent uploads, cancellation, and explicit cleanup.
- Direct SSH, ProxyJump, and targeted paste in supported tmux sessions.

## Next: connect normally

Add optional, reversible integration for interactive `ssh` launches in Windows Terminal, so users no longer type a separate executable path. Preserve normal OpenSSH behavior for commands, tunnels, scripts, and unsupported options. Provide a clear way to bypass or uninstall the integration.

This needs careful argument handling and tests for aliases, identity files, jump hosts, authentication prompts, and SSH used by other programs. The current wrapper accepts only a subset of interactive SSH options; it must not silently replace all `ssh` invocations.

## Background Windows app

Build an optional app that starts at sign-in, owns the paste hook, and coordinates active sessions. Investigate moving the current per-session bridge into this app while keeping file transfer and terminal transport separated from its UI.

Work to design and validate:

- A local session registry with authenticated registration and cleanup when a session exits.
- Access-restricted local IPC; no unauthenticated clipboard or transfer endpoint.
- Reliable routing across windows, tabs, panes, focus changes, and process restarts.
- User-controlled startup, pause, diagnostics, and uninstall.
- Clipboard access only when pasting, rather than continuously collecting clipboard contents.

An always-running app by itself does not identify the remote destination of every terminal. The session integration still needs to establish where a paste belongs and how the file will be transferred.

## More terminal clients and environments

Explore an adapter interface for Windows Terminal, VS Code, WSL, and other SSH clients. Each adapter should explain how it identifies the focused session, authenticates the transfer, and inserts a path without running it. Add support only after those behaviors can be tested.

Nested SSH, containers, different remote users, custom tmux sockets, and additional remote operating systems need explicit destination handling. They should not inherit an outer session's upload location by assumption.

## Reliability and distribution

- Installer and upgrade experience, including remote helper version management.
- Repeatable release builds and downloadable packages.
- Broader manual compatibility results and live arm64 testing.
- Recovery from interrupted transfers and clearer failure diagnostics.
- Directory transfer and virtual clipboard formats after their semantics are defined.

Propose a focused piece of this work in an issue, with the environment you can test. See [CONTRIBUTING.md](../CONTRIBUTING.md) for development instructions.

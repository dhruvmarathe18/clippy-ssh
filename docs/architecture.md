# Architecture

## Current data flow

```text
Windows clipboard
       | Ctrl+V in the registered Windows Terminal pane
       v
clippy-background.exe: hook, session registry, pause/resume
       | local authenticated named pipe: paste event, no file payload
       v
clippy.exe: focus check, clipboard snapshot, transfer worker
       | bounded protocol over native ssh.exe stdin/stdout
       v
clippy-agent: verify files, finalize batch, check destination
       | PTY input or a targeted tmux paste buffer
       v
Remote shell or terminal application receives quoted paths
```

The Windows client owns the SSH process it starts. The remote helper creates a PTY for the login shell. Terminal bytes, resize events, and file-transfer messages share the authenticated SSH connection. No remote listening service is required.

## Code map

| Area | Source | Responsibility |
| --- | --- | --- |
| CLI | `src/main.rs` | Setup, connect, diagnostics, receipt, and cleanup commands |
| SSH | `src/ssh.rs` | Supported argument parsing, OpenSSH launch, helper installation |
| Windows bridge | `src/windows/bridge.rs` | Pane registration, focus checks, local IPC, transient status |
| Background | `src/windows/background.rs` | Shared hook, tray controls, session registry and routing |
| Local IPC | `src/windows/ipc.rs` | Logon-restricted named pipe and process identity checks |
| Installation | `src/windows/install.rs` | Versioned binaries, marked profile blocks, startup and uninstall |
| Automatic SSH | `src/windows/automatic.rs` | Conservative interactive routing and native SSH passthrough |
| Clipboard | `src/windows/clipboard.rs` | File/image snapshots and clipboard format priority |
| Client | `src/windows/client.rs` | Terminal I/O, upload worker, cancellation, receipts |
| Protocol | `src/protocol.rs` | Versioned messages and bounded framing |
| Helper | `src/agent.rs` | PTY lifecycle, foreground process identity, tmux routing |
| User switches | `src/agent/destination.rs`, `src/agent/user_upload.rs` | Foreground sudo/su chain, destination UID, private receiver under that user |
| Storage | `src/transfer.rs` | Private staging, filename validation, integrity, commit, cleanup |
| Terminal | `src/terminal.rs` | Shell quoting and bracketed-paste mode tracking |
| Live tests | `tests/ssh_live.rs` | Isolated remote transfer and destination tests |

## Paste lifecycle

1. The background hook recognizes a rich clipboard paste for exactly one registered, focused session with a fresh heartbeat. Text-only paste remains with Windows Terminal.
2. A worker rechecks focus and the clipboard sequence before taking a snapshot. The low-level keyboard callback does not read clipboard payloads or perform network operations.
3. The helper snapshots the remote foreground process and, when applicable, the exact tmux pane. Files are written into a private staging batch.
4. File bytes are streamed in bounded chunks. The helper verifies size and SHA-256 before finalizing the batch.
5. The helper rechecks the destination before inserting shell-quoted paths without a newline. A changed destination keeps the saved files but prevents insertion.
6. Success dismisses progress. The client records the latest completed paths locally for `clippy last`.

During an upload, terminal input pauses and typing is discarded. Ctrl+C requests cancellation. Cancelled or failed staging batches are discarded; completed uploads persist until explicit cleanup.

For sudo/su, the helper follows an unambiguous child chain through the monitors and checks the foreground process on the nested PTY. PID, start time, and effective UID are included in the destination snapshot. A separate receiver starts through noninteractive sudo, resolves the destination home from the passwd database, and sets supplementary groups, GID, and UID before accepting upload frames. It reuses the same verification and atomic storage code. The original user's private home does not need broader permissions. If the switched shell exits before commit, files remain under the selected user's ownership and no path is inserted into the parent shell. Containers, nested SSH, and tmux reached through a user switch remain explicit errors.

Windows raw-console mode clears inherited mouse and window-input flags. In ConPTY, leaving mouse input enabled while disabling Quick Edit requests unsolicited mouse reports, which an ordinary shell displays as numbers. Connect/disconnect also resets terminal mouse modes; subsequent remote TUI mode requests pass through unchanged. The output worker stops before console cleanup so late output cannot re-enable modes after exit.

## Protocol changes

The transport begins with a magic prefix and a `Hello` message carrying protocol and package versions. Both peers must agree. Changing message layouts requires a protocol version change and a matching helper build. Frames are capped before allocation; transfer chunks are capped separately.

## Background app and automatic SSH

The background app has no SSH keys or file payloads. Each client reports focus and sends completion notifications over a bounded local protocol. The named pipe uses the current logon SID in its DACL, rejects remote connections, and verifies kernel-reported peer process IDs against the expected sibling executables. Clients prohibit server impersonation beyond identification. The hook queues events without blocking on IPC, payload reads, or UI Automation.

Clients recheck focus and the clipboard sequence before taking a snapshot. An ambiguous or stale registration is not selected. Pipe disconnect removes a registration; background exit leaves the SSH processes intact. Clients retry registration after the app is restarted.

Each bridge worker owns its pipe and alternates a focus/completion poll with one response. The hook queues paste/cancel events; the next poll delivers one event or an idle reply. Synchronous Windows pipe handles are never cloned into concurrent read/write loops, which would block focus updates behind a pending read. The runtime check exercises 100 consecutive exchanges as well as registration and disconnect.

The optional PowerShell `ssh` function calls `clippy ssh-auto`. Redirected streams, other terminal hosts, unsupported arguments, and incompatible `ssh -G` configuration are forwarded to native OpenSSH. Supported Linux shell connections get a matching remote helper before the normal client starts. `ssh.exe` always remains an explicit bypass.

Adapters for other terminal clients still need reliable focused-session and remote-destination identification. That work is described in the [roadmap](roadmap.md).

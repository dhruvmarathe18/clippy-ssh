# Architecture

## Current data flow

```text
Windows clipboard
       | Ctrl+V in the registered Windows Terminal pane
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
| Windows bridge | `src/windows/bridge.rs` | Pane registration, focus checks, paste hook, transient status |
| Clipboard | `src/windows/clipboard.rs` | File/image snapshots and clipboard format priority |
| Client | `src/windows/client.rs` | Terminal I/O, upload worker, cancellation, receipts |
| Protocol | `src/protocol.rs` | Versioned messages and bounded framing |
| Helper | `src/agent.rs` | PTY lifecycle, foreground process identity, tmux routing |
| Storage | `src/transfer.rs` | Private staging, filename validation, integrity, commit, cleanup |
| Terminal | `src/terminal.rs` | Shell quoting and bracketed-paste mode tracking |
| Live tests | `tests/ssh_live.rs` | Isolated remote transfer and destination tests |

## Paste lifecycle

1. The local bridge recognizes a rich clipboard paste in its registered, focused pane. Text-only paste remains with Windows Terminal.
2. A worker rechecks focus and the clipboard sequence before taking a snapshot. The low-level keyboard callback does not read clipboard payloads or perform network operations.
3. The helper snapshots the remote foreground process and, when applicable, the exact tmux pane. Files are written into a private staging batch.
4. File bytes are streamed in bounded chunks. The helper verifies size and SHA-256 before finalizing the batch.
5. The helper rechecks the destination before inserting shell-quoted paths without a newline. A changed destination keeps the saved files but prevents insertion.
6. Success dismisses progress. The client records the latest completed paths locally for `clippy last`.

During an upload, terminal input pauses and typing is discarded. Ctrl+C requests cancellation. Cancelled or failed staging batches are discarded; completed uploads persist until explicit cleanup.

## Protocol changes

The transport begins with a magic prefix and a `Hello` message carrying protocol and package versions. Both peers must agree. Changing message layouts requires a protocol version change and a matching helper build. Frames are capped before allocation; transfer chunks are capped separately.

## Background app direction

Currently the bridge lives inside each `clippy ssh` process. A future background app would coordinate those session connections through a restricted local interface, with terminal adapters establishing the focused destination. Moving the hook into a persistent process does not by itself grant access to an unrelated SSH client's connection. The open design work is described in the [roadmap](roadmap.md).

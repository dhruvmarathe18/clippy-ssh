# Security

Clippy SSH is an early prototype. Security fixes target the current development version; there is no long-term support branch yet.

## Report a vulnerability

Use [GitHub private vulnerability reporting](https://github.com/dhruvmarathe18/clippy-ssh/security/advisories/new) to contact the maintainer privately. Include the affected version, operating systems, reproduction steps, and expected impact. Use generated data rather than real clipboard contents or credentials.

Do not publish working exploit details or private connection data in a public issue while a report is being assessed.

## Areas of interest

- Upload routing to the wrong terminal, process, user, or host.
- Clipboard access without a paste action or outside the registered pane.
- Path traversal, symlink handling, overwrite, and cleanup boundaries.
- Protocol framing, resource exhaustion, malformed inputs, and cancellation races.
- Host-key verification, authentication handling, helper installation, and future local IPC.

Clippy uses native OpenSSH for authentication and encryption. It does not store SSH passwords or private keys. Uploaded files and local path receipts persist by design; see the [README](README.md#files-and-retention).

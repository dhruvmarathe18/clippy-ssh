# Contributing to Clippy SSH

Thanks for helping make clipboard transfer over SSH easier. This is a working Windows-to-Linux prototype with a broader background-app workflow still to build.

## Pick a change

Check the [roadmap](docs/roadmap.md) and existing issues before starting a larger change. Open an issue to discuss terminal adapters, protocol changes, or a new background-process design. Small fixes, documentation improvements, and reproducible bug reports can go straight to a pull request.

Useful first contributions include documenting a tested terminal/shell combination, improving diagnostic messages, and making installation easier. Do not mark a platform as supported based only on a successful cross-compilation.

## Development setup

Use stable Rust. For Windows, the MSVC toolchain and Visual C++ build tools are the recommended starting point. Run the client from a PowerShell tab in Windows Terminal. Linux can build and test the helper and shared modules; it cannot run the Windows clipboard client.

```text
cargo test --locked
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
```

The live SSH tests skip unless `CLIPPY_TEST_SSH_ARGS` is set. CI runs the ordinary tests and lint checks on Windows and Linux without any private SSH credentials.

To build the Windows package, run `./scripts/package.ps1` from PowerShell. It builds the Linux x86_64 and arm64 helpers, embeds them in the Windows client, and writes `dist/clippy-windows-x64.zip`.

For a Linux helper built directly on Linux:

```sh
cargo build --locked --release --bin clippy-agent
```

Transfer that artifact to your Windows development machine and use `clippy setup --agent PATH user@test-host`, or use the static helpers produced by the Windows packaging script. The client and helper versions must match.

The `.tools` directory is a maintainer-local toolchain cache, not a project dependency. Contributors with a normal Rust installation do not need `scripts/env.ps1`.

## Live tests

Use a Linux test account you control, with `tmux`, `python3`, `sha256sum`, and the matching Clippy helper installed. The test runner uses noninteractive authentication, so configure an SSH key or agent before starting it.

```powershell
$env:CLIPPY_TEST_SSH_ARGS = '["-o","BatchMode=yes","-i","C:\\path\\to\\test-key","user@test-host"]'
cargo test --locked --test ssh_live -- --nocapture --test-threads=1
```

Tests create their own fixture directories and isolated tmux servers. They remove only their own generated fixtures and completed batches. Never adapt a test to terminate an existing user session or remove an unbounded remote directory.

Protocol tests do not prove keyboard routing or popup behavior. For Windows bridge changes, record relevant manual results from the [validation checklist](docs/validation.md), including any cases you could not run.

## Design constraints

- Transfer through authenticated SSH; preserve host-key verification and OpenSSH's authentication flow.
- Read clipboard contents only in response to a paste action. Keep ordinary text paste native.
- Identify the exact focused session and remote destination. If identity is uncertain, do not guess.
- Verify file sizes and hashes before inserting paths. Never automatically press Enter.
- Keep files private and avoid overwriting existing destinations. Cleanup must stay inside validated Clippy batches.
- Keep credentials, clipboard contents, local receipts, and private host configuration out of commits and logs.

## Pull requests

Describe the user-visible problem, the resulting behavior, and how you tested it. Keep unrelated changes separate. Add tests when they exercise a meaningful failure case or behavior; a documentation-only edit does not need a new test.

Include screenshots for visible UI changes when you can capture them without exposing personal terminal content. Report security vulnerabilities through [SECURITY.md](SECURITY.md), rather than a public issue.

Contributions are licensed under the project's [MIT license](LICENSE).

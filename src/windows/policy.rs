use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{os::windows::process::CommandExt, path::PathBuf, process::Command};
use windows::Win32::System::Threading::CREATE_NO_WINDOW;

#[derive(Debug, Deserialize)]
struct Policies {
    machine_policy: String,
    user_policy: String,
    current_user: String,
    local_machine: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Change {
    shell: PathBuf,
    previous: String,
}

fn run(shell: &PathBuf, script: &str) -> Result<String> {
    let output = Command::new(shell)
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            script,
        ])
        .env_remove("PSExecutionPolicyPreference")
        // A parent PowerShell 7 process exports its module paths. Windows
        // PowerShell must rebuild its own paths instead of loading .NET Core
        // versions of Microsoft.PowerShell.Security/Utility into .NET Framework.
        .env_remove("PSModulePath")
        .creation_flags(CREATE_NO_WINDOW.0)
        .output()
        .context("inspect PowerShell profile policy")?;
    ensure!(
        output.status.success(),
        "PowerShell configuration failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(String::from_utf8(output.stdout)?
        .trim_start_matches('\u{feff}')
        .trim()
        .to_owned())
}

fn inspect(shell: &PathBuf) -> Result<Policies> {
    let text = run(
        shell,
        "$ErrorActionPreference='Stop'; [ordered]@{machine_policy=[string](Get-ExecutionPolicy -Scope MachinePolicy); user_policy=[string](Get-ExecutionPolicy -Scope UserPolicy); current_user=[string](Get-ExecutionPolicy -Scope CurrentUser); local_machine=[string](Get-ExecutionPolicy -Scope LocalMachine)} | ConvertTo-Json -Compress",
    )?;
    Ok(serde_json::from_str(&text)?)
}

fn needs_change(policies: &Policies, enable_default: bool) -> Result<bool> {
    let effective = [
        &policies.machine_policy,
        &policies.user_policy,
        &policies.current_user,
        &policies.local_machine,
    ]
    .into_iter()
    .find(|p| p.as_str() != "Undefined");
    match effective.map(String::as_str) {
        Some("RemoteSigned" | "Unrestricted" | "Bypass") => Ok(false),
        None if enable_default => Ok(true),
        None => bail!(
            "PowerShell profiles are disabled by the Windows default. Use the Clippy installer, or run clippy install --enable-profiles to enable locally created profiles for this user."
        ),
        Some(value) => bail!(
            "PowerShell policy is {value}, which blocks Clippy's unsigned profile integration. Setup will not override an explicit or organization policy. Contact your administrator or use clippy ssh explicitly."
        ),
    }
}

pub fn plan(enable_default: bool) -> Result<Vec<Change>> {
    let system = PathBuf::from(std::env::var_os("SystemRoot").context("SystemRoot missing")?)
        .join("System32/WindowsPowerShell/v1.0/powershell.exe");
    let mut shells = vec![system];
    if let Some(path) = std::env::var_os("PATH") {
        if let Some(pwsh) = std::env::split_paths(&path)
            .map(|p| p.join("pwsh.exe"))
            .find(|p| p.is_file())
        {
            shells.push(pwsh);
        }
    }
    let mut changes = Vec::new();
    for shell in shells {
        let policies = inspect(&shell)?;
        if needs_change(&policies, enable_default)
            .with_context(|| format!("{}", shell.display()))?
        {
            changes.push(Change {
                shell,
                previous: policies.current_user,
            });
        }
    }
    Ok(changes)
}

pub fn apply(changes: &[Change], applied: &mut Vec<Change>) -> Result<()> {
    for change in changes {
        // All policies are checked before any change; inspect again to avoid
        // overwriting a policy another process explicitly set in the meantime.
        let policies = inspect(&change.shell)?;
        if needs_change(&policies, true)? {
            applied.push(change.clone());
            run(
                &change.shell,
                "$ErrorActionPreference='Stop'; Set-ExecutionPolicy -Scope CurrentUser -ExecutionPolicy RemoteSigned -Force; if ((Get-ExecutionPolicy) -ne 'RemoteSigned') { throw 'PowerShell profiles remain blocked' }",
            )?;
        }
    }
    Ok(())
}

pub fn restore(changes: &[Change]) -> Result<()> {
    for change in changes.iter().rev() {
        ensure!(
            change.previous == "Undefined",
            "unexpected saved PowerShell policy"
        );
        if !change.shell.is_file() {
            continue;
        }
        if inspect(&change.shell)?.current_user == "RemoteSigned" {
            run(
                &change.shell,
                "$ErrorActionPreference='Stop'; Set-ExecutionPolicy -Scope CurrentUser -ExecutionPolicy Undefined -Force",
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn defaults() -> Policies {
        Policies {
            machine_policy: "Undefined".into(),
            user_policy: "Undefined".into(),
            current_user: "Undefined".into(),
            local_machine: "Undefined".into(),
        }
    }
    #[test]
    fn setup_enables_only_unconfigured_policy() {
        assert!(needs_change(&defaults(), true).unwrap());
        assert!(needs_change(&defaults(), false).is_err());
        let mut p = defaults();
        p.current_user = "RemoteSigned".into();
        assert!(!needs_change(&p, true).unwrap());
        for value in ["Restricted", "AllSigned"] {
            p.current_user = value.into();
            assert!(needs_change(&p, true).is_err());
            p = defaults();
            p.local_machine = value.into();
            assert!(needs_change(&p, true).is_err());
            p = defaults();
            p.machine_policy = value.into();
            p.current_user = "RemoteSigned".into();
            assert!(needs_change(&p, true).is_err());
        }
    }
}

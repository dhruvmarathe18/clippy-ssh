use anyhow::{Result, ensure};

pub const START: &str = "# >>> Clippy SSH managed integration >>>";
pub const END: &str = "# <<< Clippy SSH managed integration <<<";

/// Remove only our exact, single marker block. Refuse ambiguous/corrupted markers.
pub fn profile_block(original: &str, script: Option<&str>) -> Result<String> {
    let mut result = original.to_string();
    let starts: Vec<_> = original.match_indices(START).collect();
    let ends: Vec<_> = original.match_indices(END).collect();
    ensure!(
        starts.len() == ends.len() && starts.len() <= 1,
        "ambiguous Clippy profile markers; profile was not changed"
    );
    if let (Some((start, _)), Some((end, _))) = (starts.first(), ends.first()) {
        ensure!(
            start < end
                && (*start == 0 || original.as_bytes()[start - 1] == b'\n')
                && original[start + START.len()..].starts_with(['\r', '\n'])
                && original.as_bytes()[end - 1] == b'\n'
                && (end + END.len() == original.len()
                    || original[end + END.len()..].starts_with(['\r', '\n'])),
            "invalid Clippy profile block"
        );
        let mut after = end + END.len();
        if original[after..].starts_with("\r\n") {
            after += 2;
        } else if original[after..].starts_with('\n') {
            after += 1;
        }
        result.replace_range(*start..after, "");
    }
    if let Some(script) = script {
        let newline = if original.contains("\r\n") || original.is_empty() {
            "\r\n"
        } else {
            "\n"
        };
        if !result.is_empty() && !result.ends_with('\n') {
            result.push_str(newline);
        }
        result.push_str(&format!(
            "{START}{newline}. '{}'{newline}{END}{newline}",
            script.replace('\'', "''")
        ));
    }
    Ok(result)
}

pub fn automatic_config_supported(config: &str) -> bool {
    let mut hostname = false;
    for line in config.lines() {
        let Some((key, value)) = line.split_once(' ') else {
            continue;
        };
        let value = value.trim();
        match key {
            "hostname" => hostname = !value.is_empty(),
            "remotecommand" | "localcommand" if value != "none" => return false,
            "sessiontype" if value != "default" => return false,
            "stdinnull" | "forkafterauthentication" | "permitlocalcommand" if value != "no" => {
                return false;
            }
            "requesttty" if !["auto", "yes", "force"].contains(&value) => return false,
            "localforward" | "remoteforward" | "dynamicforward" => return false,
            _ => {}
        }
    }
    hostname
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn profile_install_update_uninstall_preserves_custom_content() {
        let original = "# café\r\nfunction prompt { 'mine> ' }\r\n";
        let first = profile_block(original, Some("C:\\a b's\\ssh.ps1")).unwrap();
        assert!(first.contains("a b''s"));
        let second = profile_block(&first, Some("C:\\updated\\ssh.ps1")).unwrap();
        assert_eq!(second.matches(START).count(), 1);
        assert_eq!(profile_block(&second, None).unwrap(), original);
    }
    #[test]
    fn ambiguous_profile_is_not_rewritten() {
        assert!(profile_block(START, None).is_err());
        assert!(profile_block(&format!("{END}\n{START}\n"), None).is_err());
        assert!(profile_block(&format!("{START}\n{END}\n{START}\n{END}"), None).is_err());
    }
    #[test]
    fn automatic_mode_respects_effective_ssh_session_settings() {
        let default = "hostname example.org\nremotecommand none\nsessiontype default\nrequesttty auto\nstdinnull no\nforkafterauthentication no\npermitlocalcommand no\n";
        assert!(automatic_config_supported(default));
        for setting in [
            "remotecommand uptime",
            "localcommand echo connected",
            "sessiontype none",
            "stdinnull yes",
            "forkafterauthentication yes",
            "requesttty no",
            "localforward 8080 localhost:80",
            "remoteforward 9000 localhost:90",
            "dynamicforward 1080",
            "permitlocalcommand yes",
        ] {
            assert!(
                !automatic_config_supported(&format!("{default}{setting}\n")),
                "{setting}"
            );
        }
        assert!(!automatic_config_supported(""));
    }
}

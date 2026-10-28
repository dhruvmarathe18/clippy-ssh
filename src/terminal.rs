#[derive(Default)]
pub struct PasteMode {
    pending: Vec<u8>,
    pub enabled: bool,
}
impl PasteMode {
    pub fn observe(&mut self, bytes: &[u8]) {
        for &b in bytes {
            if b == 27 {
                self.pending.clear();
                self.pending.push(b);
                continue;
            }
            if self.pending.is_empty() {
                continue;
            }
            self.pending.push(b);
            if self.pending.len() == 2 && b != b'[' {
                self.pending.clear();
                continue;
            }
            if self.pending.len() > 2 && (0x40..=0x7e).contains(&b) {
                if self.pending.starts_with(b"\x1b[?") && (b == b'h' || b == b'l') {
                    let p = &self.pending[3..self.pending.len() - 1];
                    if p.split(|&v| v == b';').any(|v| v == b"2004") {
                        self.enabled = b == b'h';
                    }
                }
                self.pending.clear();
            }
            if self.pending.len() > 128 {
                self.pending.clear();
            }
        }
    }
    pub fn wrap(&self, text: &str) -> Vec<u8> {
        if self.enabled {
            [b"\x1b[200~".as_slice(), text.as_bytes(), b"\x1b[201~"].concat()
        } else {
            text.as_bytes().to_vec()
        }
    }
}
pub fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}
pub fn paths_text(paths: &[String]) -> String {
    paths
        .iter()
        .map(|p| shell_quote(p))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn split_escape_sequences() {
        let mut p = PasteMode::default();
        for b in b"\x1b[?1;2004h" {
            p.observe(&[*b]);
        }
        assert!(p.enabled);
        p.observe(b"\x1b[?2004l");
        assert!(!p.enabled);
    }
    #[test]
    fn no_enter_and_quotes() {
        let s = paths_text(&["/home/u/a b's.png".into()]);
        assert_eq!(s, "'/home/u/a b'\\''s.png'");
        assert!(!s.contains('\n'));
    }
}

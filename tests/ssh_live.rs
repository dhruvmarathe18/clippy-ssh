// Opt-in: CLIPPY_TEST_SSH_ARGS is a JSON array of SSH options followed by host.
// Every test artifact has a unique ID; no existing tmux server or user file is changed.
use clippy_ssh::{
    protocol::{self, FileSpec, Message},
    ssh::Connection,
    terminal::shell_quote,
};
use sha2::{Digest, Sha256};
use std::{
    process::{Child, ChildStdin, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

struct Session {
    child: Child,
    input: ChildStdin,
    events: mpsc::Receiver<Message>,
    connection: Connection,
    fixture: String,
    uploads: Vec<(String, Option<String>)>,
    upload_user: Option<String>,
}
impl Session {
    fn open() -> Option<Self> {
        let args = std::env::var("CLIPPY_TEST_SSH_ARGS").ok()?;
        let connection =
            Connection::parse(&serde_json::from_str::<Vec<String>>(&args).unwrap()).unwrap();
        let fixture = format!(".local/share/clippy/integration/{}", uuid::Uuid::new_v4());
        let script = format!(
            "umask 077; mkdir -p \"$HOME/{fixture}/tmux\"; export TMUX_TMPDIR=\"$HOME/{fixture}/tmux\"; exec \"$HOME/.local/lib/clippy/{}/clippy-agent\" serve",
            env!("CARGO_PKG_VERSION")
        );
        let mut child = connection
            .command(&script)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let mut output = child.stdout.take().unwrap();
        protocol::handshake(&mut output).unwrap();
        assert!(matches!(
            protocol::receive(&mut output).unwrap(),
            Message::Hello {
                protocol: protocol::VERSION,
                ..
            }
        ));
        let (tx, events) = mpsc::channel();
        std::thread::spawn(move || {
            while let Ok(m) = protocol::receive(&mut output) {
                if tx.send(m).is_err() {
                    break;
                }
            }
        });
        let mut session = Self {
            child,
            input,
            events,
            connection,
            fixture,
            uploads: vec![],
            upload_user: None,
        };
        session.send(Message::Start {
            term: "xterm-256color".into(),
            cols: 100,
            rows: 30,
        });
        session.until(|m| matches!(m, Message::Started));
        session.ready();
        Some(session)
    }
    fn send(&mut self, m: Message) {
        protocol::send(&mut self.input, &m).unwrap();
    }
    fn until(&mut self, mut f: impl FnMut(&Message) -> bool) -> Message {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let m = self
                .events
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .expect("agent timed out");
            if f(&m) {
                return m;
            }
        }
    }
    fn output(&mut self, needle: &str) {
        let mut data = String::new();
        self.until(|m| {
            if let Message::Output(b) = m {
                data.push_str(&String::from_utf8_lossy(b));
            }
            data.contains(needle)
        });
    }
    fn ready(&mut self) {
        self.send(Message::Input(
            b"\x15printf '\\137CLIPPY_READY\\137\\n'\r".to_vec(),
        ));
        self.output("_CLIPPY_READY_");
        std::thread::sleep(Duration::from_millis(150));
    }
    fn begin(&mut self, specs: Vec<FileSpec>) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        self.send(Message::Begin {
            id: id.clone(),
            files: specs,
        });
        let m = self.until(|m| matches!(m, Message::Accepted { .. } | Message::Error { .. }));
        assert!(matches!(m, Message::Accepted { .. }), "{m:?}");
        id
    }
    fn complete(&mut self, id: String) -> Vec<String> {
        self.send(Message::Commit { id: id.clone() });
        match self.until(|m| matches!(m, Message::Completed { .. } | Message::Error { .. })) {
            Message::Completed {
                paths,
                inserted,
                detail,
                ..
            } => {
                assert!(inserted, "{detail}");
                self.uploads.push((id, self.upload_user.clone()));
                paths
            }
            m => panic!("{m:?}"),
        }
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        let _ = protocol::send(&mut self.input, &Message::Shutdown);
        let _ = self.child.kill();
        let _ = self.child.wait();
        // Only this test's random fixture and recorded upload IDs can be removed.
        let mut script = format!(
            "export TMUX_TMPDIR=\"$HOME/{}/tmux\"; tmux kill-server 2>/dev/null || true; rm -rf -- \"$HOME/{}\"",
            self.fixture, self.fixture
        );
        for (id, user) in &self.uploads {
            assert!(uuid::Uuid::parse_str(id).is_ok());
            let cleanup = format!("rm -rf -- \"$HOME/.local/share/clippy/uploads/{id}\"");
            if let Some(user) = user {
                script.push_str(&format!(
                    "; sudo -n -H -u {} -- sh -c {}",
                    shell_quote(user),
                    shell_quote(&cleanup)
                ));
            } else {
                script.push_str(&format!("; {cleanup}"));
            }
        }
        let _ = self.connection.command(&script).status();
    }
}

#[test]
fn live_verified_upload_and_cancel() {
    let Some(mut s) = Session::open() else {
        eprintln!("SKIP: set CLIPPY_TEST_SSH_ARGS for a live SSH test");
        return;
    };
    let data = b"Clippy binary\0\xff\n";
    let id = s.begin(vec![
        FileSpec {
            name: "a b's.bin".into(),
            size: data.len() as u64,
        },
        FileSpec {
            name: "empty".into(),
            size: 0,
        },
    ]);
    s.send(Message::Chunk {
        index: 0,
        offset: 0,
        data: data.to_vec(),
    });
    s.send(Message::FinishFile {
        index: 0,
        sha256: Sha256::digest(data).into(),
    });
    s.send(Message::FinishFile {
        index: 1,
        sha256: Sha256::digest([]).into(),
    });
    let paths = s.complete(id);
    assert_eq!(paths.len(), 2);
    s.send(Message::Input(
        format!("\x15sha256sum -- {}\r", shell_quote(&paths[0])).into_bytes(),
    ));
    s.output(&hex::encode(Sha256::digest(data)));
    s.ready();
    let id = s.begin(vec![FileSpec {
        name: "cancel.bin".into(),
        size: 100,
    }]);
    s.send(Message::Chunk {
        index: 0,
        offset: 0,
        data: vec![1; 20],
    });
    s.send(Message::Cancel { id });
    assert!(
        matches!(s.until(|m|matches!(m,Message::Completed{..})),Message::Completed{inserted:false,paths,..} if paths.is_empty())
    );
    s.ready();
    let id = s.begin(vec![FileSpec {
        name: "corrupt.bin".into(),
        size: 1,
    }]);
    s.send(Message::Chunk {
        index: 0,
        offset: 0,
        data: vec![42],
    });
    s.send(Message::FinishFile {
        index: 0,
        sha256: [0; 32],
    });
    assert!(
        matches!(s.until(|m|matches!(m,Message::Error{..})),Message::Error{id:Some(i),..} if i==id)
    );
    s.ready();
}

#[test]
fn live_shell_path_insertion_preserves_command_without_enter() {
    shell_path_insertion(false);
}
#[test]
fn live_tmux_path_insertion_preserves_command_without_enter() {
    shell_path_insertion(true);
}
fn shell_path_insertion(tmux: bool) {
    let Some(mut s) = Session::open() else {
        return;
    };
    if tmux {
        s.send(Message::Input(
            b"tmux new-session -s clippy-test\r".to_vec(),
        ));
        std::thread::sleep(Duration::from_secs(2));
        s.ready();
    }
    s.send(Message::Input(
        b"printf '\\137PASTE\\137%s\\137END\\137\\n' ".to_vec(),
    ));
    let id = s.begin(vec![FileSpec {
        name: "clipboard.png".into(),
        size: 3,
    }]);
    s.send(Message::Chunk {
        index: 0,
        offset: 0,
        data: b"abc".to_vec(),
    });
    s.send(Message::FinishFile {
        index: 0,
        sha256: Sha256::digest(b"abc").into(),
    });
    let paths = s.complete(id);
    let mut output = String::new();
    while let Ok(message) = s.events.recv_timeout(Duration::from_millis(250)) {
        if let Message::Output(bytes) = message {
            output.push_str(&String::from_utf8_lossy(&bytes));
        }
        assert!(!output.contains("_PASTE_"), "paste executed the command");
    }
    s.send(Message::Input(b" | sha256sum\r".to_vec()));
    s.output(&hex::encode(Sha256::digest(
        format!("_PASTE_{}_END_\n", paths[0]).as_bytes(),
    )));
}
#[test]
fn live_tmux_paste() {
    let Some(mut s) = Session::open() else {
        return;
    };
    s.send(Message::Input(
        b"tmux new-session -s clippy-test\r".to_vec(),
    ));
    std::thread::sleep(Duration::from_secs(2));
    s.ready();
    let data = b"tmux\0image";
    let id = s.begin(vec![FileSpec {
        name: "clipboard.png".into(),
        size: data.len() as u64,
    }]);
    s.send(Message::Chunk {
        index: 0,
        offset: 0,
        data: data.to_vec(),
    });
    s.send(Message::FinishFile {
        index: 0,
        sha256: Sha256::digest(data).into(),
    });
    let paths = s.complete(id);
    s.send(Message::Input(
        format!("\x15sha256sum -- {}\r", shell_quote(&paths[0])).into_bytes(),
    ));
    s.output(&hex::encode(Sha256::digest(data)));
}

#[test]
fn live_application_receives_path() {
    let Some(mut s) = Session::open() else {
        return;
    };
    s.send(Message::Input(b"python3 -c 'print(chr(65)+chr(80)+chr(80), flush=True); x=input(); print(chr(82)+chr(88)+chr(58)+x, flush=True)'\r".to_vec()));
    s.output("APP\r\n");
    let id = s.begin(vec![FileSpec {
        name: "app-input.txt".into(),
        size: 0,
    }]);
    s.send(Message::FinishFile {
        index: 0,
        sha256: Sha256::digest([]).into(),
    });
    let paths = s.complete(id);
    assert_eq!(paths.len(), 1);
    s.send(Message::Input(b"\r".to_vec()));
    s.output(&format!("RX:{}", shell_quote(&paths[0])));
    s.ready();
}

#[test]
fn live_tmux_target_change_keeps_files_without_pasting() {
    let Some(mut s) = Session::open() else {
        return;
    };
    s.send(Message::Input(
        b"tmux new-session -s clippy-test\r".to_vec(),
    ));
    std::thread::sleep(Duration::from_secs(2));
    s.ready();
    let id = s.begin(vec![FileSpec {
        name: "changed-pane.txt".into(),
        size: 3,
    }]);
    s.send(Message::Chunk {
        index: 0,
        offset: 0,
        data: b"abc".to_vec(),
    });
    s.send(Message::FinishFile {
        index: 0,
        sha256: Sha256::digest(b"abc").into(),
    });
    let change = format!(
        "export TMUX_TMPDIR=\"$HOME/{}/tmux\"; tmux split-window -v -t clippy-test",
        s.fixture
    );
    assert!(s.connection.command(&change).status().unwrap().success());
    s.send(Message::Commit { id: id.clone() });
    match s.until(|m| matches!(m, Message::Completed { .. } | Message::Error { .. })) {
        Message::Completed {
            inserted,
            paths,
            detail,
            ..
        } => {
            assert!(!inserted);
            assert_eq!(paths.len(), 1);
            assert!(detail.contains("changed"));
            s.uploads.push((id, s.upload_user.clone()));
        }
        m => panic!("{m:?}"),
    }
    s.ready();
}

#[test]
fn live_sudo_su_saves_as_foreground_user_without_enter() {
    let Ok(user) = std::env::var("CLIPPY_TEST_SWITCH_USER") else {
        return;
    };
    let Some(mut s) = Session::open() else {
        return;
    };
    let identity = s
        .connection
        .command(&format!("id -u -- {}", shell_quote(&user)))
        .output()
        .unwrap();
    assert!(identity.status.success());
    let uid = String::from_utf8(identity.stdout)
        .unwrap()
        .trim()
        .parse::<u32>()
        .unwrap();
    s.send(Message::Input(
        format!("sudo -n su -- {}\r", shell_quote(&user)).into_bytes(),
    ));
    std::thread::sleep(Duration::from_millis(750));
    s.ready();
    s.upload_user = Some(user.clone());
    s.send(Message::Input(
        b"printf '\\137USER_PATH\\137%s\\137END\\137\\n' ".to_vec(),
    ));
    let data = b"switched user\0PNG payload";
    let id = s.begin(vec![FileSpec {
        name: "a screenshot's.png".into(),
        size: data.len() as u64,
    }]);
    s.send(Message::Chunk {
        index: 0,
        offset: 0,
        data: data.to_vec(),
    });
    s.send(Message::FinishFile {
        index: 0,
        sha256: Sha256::digest(data).into(),
    });
    let paths = s.complete(id);
    let mut output = String::new();
    while let Ok(message) = s.events.recv_timeout(Duration::from_millis(200)) {
        if let Message::Output(bytes) = message {
            output.push_str(&String::from_utf8_lossy(&bytes));
        }
        assert!(
            !output.contains("_USER_PATH_"),
            "paste executed the command"
        );
    }
    s.send(Message::Input(b" | sha256sum\r".to_vec()));
    s.output(&hex::encode(Sha256::digest(format!(
        "_USER_PATH_{}_END_\n",
        paths[0]
    ))));
    s.ready();
    // Read the file from the switched shell, then verify ownership and private permissions independently.
    s.send(Message::Input(
        format!("sha256sum -- {}\r", shell_quote(&paths[0])).into_bytes(),
    ));
    s.output(&hex::encode(Sha256::digest(data)));
    let verify = s
        .connection
        .command(&format!(
            "sudo -n -H -u {} -- stat -c '%u:%a' -- {}",
            shell_quote(&user),
            shell_quote(&paths[0])
        ))
        .output()
        .unwrap();
    assert!(verify.status.success());
    assert_eq!(
        String::from_utf8(verify.stdout).unwrap().trim(),
        format!("{uid}:600")
    );
    s.ready();
    // Cancellation must remove the switched user's private staging directory.
    let cancelled = s.begin(vec![FileSpec {
        name: "cancelled.bin".into(),
        size: 20,
    }]);
    s.send(Message::Chunk {
        index: 0,
        offset: 0,
        data: vec![1; 5],
    });
    s.send(Message::Cancel {
        id: cancelled.clone(),
    });
    s.until(|m| matches!(m, Message::Completed { .. }));
    let check = format!(
        "test ! -e \"$HOME/.local/share/clippy/uploads/.partial-{cancelled}\" && test ! -e \"$HOME/.local/share/clippy/uploads/{cancelled}\""
    );
    assert!(
        s.connection
            .command(&format!(
                "sudo -n -H -u {} -- sh -c {}",
                shell_quote(&user),
                shell_quote(&check)
            ))
            .status()
            .unwrap()
            .success()
    );
    let corrupt = s.begin(vec![FileSpec {
        name: "corrupt.bin".into(),
        size: 1,
    }]);
    s.send(Message::Chunk {
        index: 0,
        offset: 0,
        data: vec![1],
    });
    s.send(Message::FinishFile {
        index: 0,
        sha256: [0; 32],
    });
    s.send(Message::Commit {
        id: corrupt.clone(),
    });
    s.until(|m| matches!(m, Message::Error { .. }));
    let check = format!(
        "test ! -e \"$HOME/.local/share/clippy/uploads/.partial-{corrupt}\" && test ! -e \"$HOME/.local/share/clippy/uploads/{corrupt}\""
    );
    assert!(
        s.connection
            .command(&format!(
                "sudo -n -H -u {} -- sh -c {}",
                shell_quote(&user),
                shell_quote(&check)
            ))
            .status()
            .unwrap()
            .success()
    );
    s.send(Message::Input(b"exit\r".to_vec()));
    std::thread::sleep(Duration::from_millis(200));
    s.upload_user = None;
    s.ready();
    // Returning to the original shell must restore its normal upload destination.
    let id = s.begin(vec![FileSpec {
        name: "back.txt".into(),
        size: 0,
    }]);
    s.send(Message::FinishFile {
        index: 0,
        sha256: Sha256::digest([]).into(),
    });
    let original = s.complete(id);
    assert_ne!(
        std::path::Path::new(&original[0]).components().nth(2),
        std::path::Path::new(&paths[0]).components().nth(2)
    );
}

#[test]
fn live_sudo_user_exit_keeps_files_without_pasting_into_parent() {
    let Ok(user) = std::env::var("CLIPPY_TEST_SWITCH_USER") else {
        return;
    };
    let Some(mut s) = Session::open() else {
        return;
    };
    s.send(Message::Input(
        format!("sudo -n su -- {}\r", shell_quote(&user)).into_bytes(),
    ));
    std::thread::sleep(Duration::from_millis(750));
    s.ready();
    s.upload_user = Some(user);
    s.send(Message::Input(
        b"printf '\\137OWN_SHELL\\137%s\\137END\\137\\n' \"$$\"\r".to_vec(),
    ));
    let mut output = String::new();
    s.until(|m| {
        if let Message::Output(b) = m {
            output.push_str(&String::from_utf8_lossy(b));
        }
        output.contains("_OWN_SHELL_") && output.contains("_END_")
    });
    let pid = output
        .split("_OWN_SHELL_")
        .nth(1)
        .unwrap()
        .split("_END_")
        .next()
        .unwrap()
        .parse::<u32>()
        .unwrap();
    s.ready();
    let id = s.begin(vec![FileSpec {
        name: "changed-user.bin".into(),
        size: 0,
    }]);
    s.send(Message::FinishFile {
        index: 0,
        sha256: Sha256::digest([]).into(),
    });
    // This PID came from the uniquely created test session, never a user session.
    assert!(
        s.connection
            .command(&format!("sudo -n kill -HUP -- {pid}"))
            .status()
            .unwrap()
            .success()
    );
    std::thread::sleep(Duration::from_millis(300));
    s.send(Message::Commit { id: id.clone() });
    match s.until(|m| matches!(m, Message::Completed { .. } | Message::Error { .. })) {
        Message::Completed {
            paths,
            inserted,
            detail,
            ..
        } => {
            assert!(!inserted);
            assert_eq!(paths.len(), 1);
            assert!(detail.contains("changed"));
            s.uploads.push((id, s.upload_user.clone()));
        }
        m => panic!("{m:?}"),
    }
    s.ready();
}

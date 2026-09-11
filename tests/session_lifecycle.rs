use std::fs;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Server {
    root: PathBuf,
    socket: PathBuf,
    child: Option<Child>,
}

impl Server {
    fn new(config: &str) -> Self {
        let root = PathBuf::from(format!(
            "/tmp/rm-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join("config/rustmux")).unwrap();
        fs::create_dir_all(root.join("workspace")).unwrap();
        fs::write(root.join("config/rustmux/config.toml"), config).unwrap();
        let socket = root
            .join(format!("rustmux-{}", unsafe { nix::libc::getuid() }))
            .join("work.sock");
        let mut server = Self {
            root,
            socket,
            child: None,
        };
        server.start();
        server
    }

    fn start(&mut self) {
        self.child = Some(
            Command::new(env!("CARGO_BIN_EXE_rustmux"))
                .arg("--server")
                .arg(&self.socket)
                .args(["80", "24", "0", "0"])
                .env("RUSTMUX_SHELL", "/bin/sh")
                .env("XDG_CONFIG_HOME", self.root.join("config"))
                .env("XDG_STATE_HOME", self.root.join("state"))
                .env("TMPDIR", &self.root)
                .env("ENV", "/dev/null")
                .current_dir(&self.root)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap(),
        );
        eventually(|| self.status().is_some());
    }

    fn status(&self) -> Option<String> {
        let mut client = UnixStream::connect(&self.socket).ok()?;
        client
            .set_read_timeout(Some(Duration::from_millis(200)))
            .ok()?;
        client.write_all(b"S").ok()?;
        let mut output = String::new();
        client.read_to_string(&mut output).ok()?;
        Some(output)
    }

    fn attach(&self) -> UnixStream {
        let mut client = UnixStream::connect(&self.socket).unwrap();
        client.write_all(&[b'R', 0, 80, 0, 24, 0, 0, 0, 0]).unwrap();
        // Drain frames concurrently so the server cannot block on a full socket.
        let mut reader = client.try_clone().unwrap();
        thread::spawn(move || {
            let _ = std::io::copy(&mut reader, &mut std::io::sink());
        });
        client
    }

    fn stop(&mut self, delete: bool) {
        if self.child.is_none() {
            if let Ok(mut client) = UnixStream::connect(&self.socket) {
                let _ = client.write_all(if delete { b"X" } else { b"Q" });
                let _ = client.shutdown(std::net::Shutdown::Write);
                let deadline = Instant::now() + Duration::from_secs(3);
                while self.socket.exists() && Instant::now() < deadline {
                    thread::sleep(Duration::from_millis(10));
                }
            }
            return;
        }
        if let Some(mut child) = self.child.take() {
            if let Ok(mut client) = UnixStream::connect(&self.socket) {
                let _ = client.write_all(if delete { b"X" } else { b"Q" });
                let _ = client.shutdown(std::net::Shutdown::Write);
                client
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let _ = client.read_to_end(&mut Vec::new());
            }
            let deadline = Instant::now() + Duration::from_secs(5);
            while child.try_wait().unwrap().is_none() {
                if Instant::now() > deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    if !thread::panicking() {
                        panic!("server did not shut down");
                    }
                    break;
                }
                thread::sleep(Duration::from_millis(10));
            }
        }
    }

    fn cli(&self, arguments: &[&str]) -> std::process::Output {
        self.cli_shell(arguments, "/bin/sh")
    }

    fn cli_shell(&self, arguments: &[&str], shell: &str) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_rustmux"))
            .args(arguments)
            .env("TMPDIR", &self.root)
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("XDG_STATE_HOME", self.root.join("state"))
            .env("RUSTMUX_SHELL", shell)
            .output()
            .unwrap()
    }

    fn snapshot(&self) -> PathBuf {
        self.root.join("state/rustmux/sessions/work.toml")
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop(false);
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn eventually(mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !predicate() {
        assert!(Instant::now() < deadline, "condition did not become true");
        thread::sleep(Duration::from_millis(20));
    }
}

fn input(client: &mut UnixStream, value: &[u8]) {
    client.write_all(b"I").unwrap();
    client
        .write_all(&(value.len() as u32).to_be_bytes())
        .unwrap();
    client.write_all(value).unwrap();
}

#[test]
fn saves_directory_and_split_on_detach_and_restores_after_restart() {
    let mut server = Server::new("autosave_interval_seconds = 30\n");
    let mut client = server.attach();
    input(&mut client, b"cd workspace; touch ready\n");
    eventually(|| server.root.join("workspace/ready").exists());
    input(&mut client, b"\x02\x10r");
    eventually(|| server.status().is_some_and(|s| s.starts_with("1\t2\t")));
    input(&mut client, b"\x02d");
    eventually(|| server.snapshot().exists());
    let snapshot: toml::Value =
        toml::from_str(&fs::read_to_string(server.snapshot()).unwrap()).unwrap();
    let panes = snapshot["tabs"][0]["panes"].as_array().unwrap();
    assert_eq!(panes.len(), 2);
    let expected = fs::canonicalize(server.root.join("workspace")).unwrap();
    for pane in panes {
        assert_eq!(PathBuf::from(pane["cwd"].as_str().unwrap()), expected);
    }
    server.stop(false);
    server.start();
    assert!(server.status().unwrap().starts_with("1\t2\t0\t"));
    let mut client = server.attach();
    input(&mut client, b"pwd > restored-directory\n");
    eventually(|| expected.join("restored-directory").exists());
    assert_eq!(
        fs::read_to_string(expected.join("restored-directory"))
            .unwrap()
            .trim(),
        expected.to_str().unwrap()
    );
    server.stop(true);
    assert!(
        !server.snapshot().exists(),
        "deleting must not autosave on shutdown"
    );
}

#[test]
fn periodic_saving_can_be_disabled_and_reenabled_by_reload() {
    let mut server = Server::new("autosave_interval_seconds = 0\n");
    let mut client = server.attach();
    input(&mut client, b"\x02d");
    eventually(|| server.status().is_some_and(|s| s.starts_with("1\t1\t0\t")));
    server.stop(false);
    assert!(!server.snapshot().exists());
    server.start();
    fs::write(
        server.root.join("config/rustmux/config.toml"),
        "autosave_interval_seconds = 1\n",
    )
    .unwrap();
    eventually(|| server.snapshot().exists());
    server.stop(true);
    assert!(!server.snapshot().exists());
}

#[test]
fn control_commands_target_panes_without_an_attached_client() {
    let server = Server::new("autosave_interval_seconds = 0\n");
    let run = |args: &[&str]| {
        let output = server.cli(args);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    };
    let new_id = run(&["split-pane", "-s", "work", "-p", "1", "--down"]);
    let new_id = new_id.trim();
    assert_ne!(new_id, "1");
    run(&[
        "send-keys",
        "-s",
        "work",
        "-p",
        new_id,
        "--literal",
        "--enter",
        "printf 'rpc-marker\\n'",
    ]);
    eventually(|| run(&["capture-pane", "-s", "work", "-p", new_id]).contains("rpc-marker"));
    let list: toml::Value = toml::from_str(&run(&["list-panes", "-s", "work", "--toml"])).unwrap();
    assert_eq!(list["panes"].as_array().unwrap().len(), 2);
    run(&["new-window", "-s", "work", "--name", "logs"]);
    run(&["save-session", "-s", "work"]);
    assert!(server.snapshot().exists());
    assert!(
        !server
            .cli(&["capture-pane", "-s", "work", "-p", "99999"])
            .status
            .success()
    );
    assert!(
        !server
            .cli(&["send-keys", "-s", "work", "-p", "1", "not-a-key"])
            .status
            .success()
    );
    assert!(server.status().unwrap().starts_with("2\t3\t0\t"));
}

#[test]
fn detached_project_layout_runs_and_restores_startup_commands() {
    let mut server = Server::new("autosave_interval_seconds = 30\n");
    server.stop(false);
    let layout = server.root.join("project.toml");
    fs::write(
        &layout,
        r#"
[[windows]]
name = "project"
[[windows.panes]]
cwd = "workspace"
command = "printf 'started\\n' >> startup-count; exec /bin/sh"
[[windows.panes]]
split = "down"
cwd = "workspace"
"#,
    )
    .unwrap();
    let output = server.cli(&[
        "new-session",
        "work",
        "--detached",
        "--layout",
        layout.to_str().unwrap(),
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(server.status().unwrap().starts_with("1\t2\t0\t"));
    let count_file = server.root.join("workspace/startup-count");
    eventually(|| count_file.exists());
    assert!(
        !server
            .cli(&[
                "new-session",
                "work",
                "--detached",
                "--layout",
                layout.to_str().unwrap()
            ])
            .status
            .success()
    );
    assert!(server.cli(&["save-session", "-s", "work"]).status.success());
    server.stop(false);
    server.start();
    eventually(|| fs::read_to_string(&count_file).unwrap().lines().count() == 2);
    server.stop(true);
    fs::write(&layout, "windows = []").unwrap();
    assert!(
        !server
            .cli(&[
                "new-session",
                "work",
                "--detached",
                "--layout",
                layout.to_str().unwrap()
            ])
            .status
            .success()
    );
    assert!(!server.socket.exists());
}

#[test]
fn startup_errors_reach_the_cli_and_leave_no_session_socket() {
    use std::os::unix::fs::PermissionsExt;
    let mut server = Server::new("autosave_interval_seconds = 0\n");
    server.stop(false);
    let shell = server.root.join("bad-shell");
    fs::write(&shell, "#!/does-not-exist/rustmux-shell\n").unwrap();
    fs::set_permissions(&shell, fs::Permissions::from_mode(0o700)).unwrap();
    let output = server.cli_shell(
        &["new-session", "work", "--detached"],
        shell.to_str().unwrap(),
    );
    assert!(!output.status.success());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(
        error.contains("could not start") && error.contains("bad-shell"),
        "{error}"
    );
    assert!(!server.socket.exists());
}

#[test]
fn pane_moves_preserve_shell_state_and_saved_layout() {
    let mut server = Server::new("autosave_interval_seconds = 0\n");
    let run = |args: &[&str]| {
        let output = server.cli(args);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    };
    run(&[
        "send-keys",
        "-s",
        "work",
        "-p",
        "1",
        "--literal",
        "--enter",
        "export RUSTMUX_MOVE_TEST=preserved; echo $$ > old-pid",
    ]);
    eventually(|| server.root.join("old-pid").exists());
    let destination = run(&["new-window", "-s", "work", "--name", "target"]);
    run(&[
        "join-pane",
        "-s",
        "work",
        "-p",
        "1",
        "--to-pane",
        destination.trim(),
    ]);
    assert!(server.status().unwrap().starts_with("1\t2\t"));
    run(&["break-pane", "-s", "work", "-p", "1", "--name", "moved"]);
    assert!(server.status().unwrap().starts_with("2\t2\t"));
    run(&[
        "send-keys",
        "-s",
        "work",
        "-p",
        "1",
        "--literal",
        "--enter",
        "echo $$ > new-pid; echo $RUSTMUX_MOVE_TEST > move-value",
    ]);
    eventually(|| server.root.join("move-value").exists());
    assert_eq!(
        fs::read_to_string(server.root.join("old-pid")).unwrap(),
        fs::read_to_string(server.root.join("new-pid")).unwrap()
    );
    assert_eq!(
        fs::read_to_string(server.root.join("move-value"))
            .unwrap()
            .trim(),
        "preserved"
    );
    assert!(
        !server
            .cli(&["join-pane", "-s", "work", "-p", "1", "--to-pane", "1"])
            .status
            .success()
    );
    run(&["save-session", "-s", "work"]);
    server.stop(false);
    server.start();
    let output = server.cli(&["list-panes", "-s", "work", "--toml"]);
    let panes: toml::Value = toml::from_str(std::str::from_utf8(&output.stdout).unwrap()).unwrap();
    assert_eq!(panes["panes"].as_array().unwrap().len(), 2);
    assert!(
        panes["panes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|pane| pane["name"].as_str() == Some("moved"))
    );
}

#[test]
fn attached_session_repaints_theme_and_keeps_last_valid_colors() {
    use std::sync::{Arc, Mutex};
    let server = Server::new("[theme]\npreset='light'\n");
    let frames = Arc::new(Mutex::new(Vec::new()));
    let received = frames.clone();
    let mut client = UnixStream::connect(&server.socket).unwrap();
    client.write_all(&[b'R', 0, 80, 0, 24, 0, 0, 0, 0]).unwrap();
    let mut reader = client.try_clone().unwrap();
    let reader_thread = thread::spawn(move || {
        let mut bytes = [0; 8192];
        loop {
            match reader.read(&mut bytes) {
                Ok(0) | Err(_) => break,
                Ok(count) => received.lock().unwrap().extend_from_slice(&bytes[..count]),
            }
        }
    });
    let contains = |needle: &str| String::from_utf8_lossy(&frames.lock().unwrap()).contains(needle);
    eventually(|| contains("48;2;245;246;250"));
    let path = server.root.join("config/rustmux/config.toml");
    fs::write(
        &path,
        "[theme.colors]\nbackground='#010203'\naccent='#040506'\n",
    )
    .unwrap();
    eventually(|| contains("48;2;1;2;3") && contains("38;2;4;5;6"));
    frames.lock().unwrap().clear();
    fs::write(&path, "[theme.colors]\naccent='invalid'\n").unwrap();
    eventually(|| contains("config reload failed"));
    assert!(
        contains("48;2;1;2;3"),
        "invalid update must retain the last valid palette"
    );
    let default_bar_style = "\x1b[0;38;2;205;214;244;48;2;30;30;46m";
    assert!(!contains(default_bar_style));
    frames.lock().unwrap().clear();
    fs::remove_file(&path).unwrap();
    eventually(|| contains(default_bar_style));
    client.shutdown(std::net::Shutdown::Both).unwrap();
    reader_thread.join().unwrap();
}

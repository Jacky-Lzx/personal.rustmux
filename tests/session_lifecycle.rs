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
    input(&mut client, b"\x02\x0fd");
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
    input(&mut client, b"\x02\x0fd");
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
fn kill_all_sessions_stops_attached_server() {
    let mut server = Server::new("autosave_interval_seconds = 0\n");
    let _client = server.attach();
    let output = server.cli(&["kill-all-sessions", "--yes"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    eventually(|| server.child.as_mut().unwrap().try_wait().unwrap().is_some());
    assert!(!server.socket.exists());
}

#[test]
fn kill_all_sessions_continues_after_stale_socket_and_preserves_snapshot() {
    let mut server = Server::new("autosave_interval_seconds = 0\n");
    let snapshot = server.snapshot();
    fs::create_dir_all(snapshot.parent().unwrap()).unwrap();
    fs::write(&snapshot, "saved snapshot sentinel").unwrap();
    let stale = server.socket.parent().unwrap().join("aaa-stale.sock");
    drop(std::os::unix::net::UnixListener::bind(stale).unwrap());
    let output = server.cli(&["kill-all-sessions", "--yes"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("aaa-stale"));
    assert!(String::from_utf8_lossy(&output.stdout).contains("killed 1 session(s)"));
    eventually(|| server.child.as_mut().unwrap().try_wait().unwrap().is_some());
    assert!(!server.socket.exists());
    assert_eq!(
        fs::read_to_string(snapshot).unwrap(),
        "saved snapshot sentinel"
    );
}

#[test]
fn open_session_manager_reloads_delete_binding() {
    use std::sync::{Arc, Mutex};
    let server = Server::new("[session_manager]\ndelete=['delete']\n");
    let screen = Arc::new(Mutex::new(vt100::Parser::new(24, 200, 0)));
    let parsed = screen.clone();
    let mut client = UnixStream::connect(&server.socket).unwrap();
    client
        .write_all(&[b'R', 0, 200, 0, 24, 0, 0, 0, 0])
        .unwrap();
    let mut reader = client.try_clone().unwrap();
    let reader_thread = thread::spawn(move || {
        let mut bytes = [0; 8192];
        loop {
            match reader.read(&mut bytes) {
                Ok(0) | Err(_) => break,
                Ok(count) => parsed.lock().unwrap().process(&bytes[..count]),
            }
        }
    });
    input(&mut client, b"\x02\x17");
    let contents = || screen.lock().unwrap().screen().contents();
    eventually(|| contents().contains("<Delete> Delete"));
    fs::write(
        server.root.join("config/rustmux/config.toml"),
        "[session_manager]\ndelete=['d']\n",
    )
    .unwrap();
    eventually(|| contents().contains("<d> Delete") && !contents().contains("<Delete> Delete"));
    // The old key must no longer delete the session.
    input(&mut client, b"\x1b[3~");
    thread::sleep(Duration::from_millis(150));
    assert!(server.status().is_some());
    input(&mut client, b"d");
    eventually(|| server.status().is_none());
    reader_thread.join().unwrap();
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

#[test]
fn second_attach_warns_without_disconnecting_resizing_or_resetting_first_client() {
    let server = Server::new("autosave_interval_seconds = 0\n");
    let mut first = server.attach();
    input(&mut first, b"stty size > before-size\n");
    eventually(|| server.root.join("before-size").exists());
    input(&mut first, b"\x02"); // Keep the first client in normal mode.

    let mut second = UnixStream::connect(&server.socket).unwrap();
    second
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    second
        .write_all(&[b'R', 0, 160, 0, 60, 0, 0, 0, 0])
        .unwrap();
    let mut response = Vec::new();
    second.read_to_end(&mut response).unwrap();
    assert_eq!(response, b"\x1b]777;rustmux-session-busy\x07");
    input(&mut first, b"c");
    eventually(|| server.status().is_some_and(|s| s.starts_with("2\t2\t1\t")));
    input(&mut first, b"stty size > after-size\n");
    eventually(|| server.root.join("after-size").exists());
    assert_eq!(
        fs::read(server.root.join("before-size")).unwrap(),
        fs::read(server.root.join("after-size")).unwrap()
    );

    // Exercise the actual CLI, including raw-mode cleanup and warning output.
    let pty = nix::pty::openpty(
        Some(&nix::pty::Winsize {
            ws_row: 24,
            ws_col: 80,
            ws_xpixel: 0,
            ws_ypixel: 0,
        }),
        None,
    )
    .unwrap();
    let slave = fs::File::from(pty.slave);
    let mut child = Command::new(env!("CARGO_BIN_EXE_rustmux"))
        .args(["attach", "work"])
        .env("TMPDIR", &server.root)
        .env("XDG_CONFIG_HOME", server.root.join("config"))
        .env_remove("RUSTMUX")
        .stdin(slave.try_clone().unwrap())
        .stdout(slave)
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("rejected client did not exit");
        }
        thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let warning = String::from_utf8_lossy(&output.stderr);
    assert!(
        warning.contains("warning: session 'work' is already attached"),
        "{warning}"
    );
    assert_eq!(warning.matches("warning:").count(), 1);
    assert!(server.status().unwrap().starts_with("2\t2\t1\t"));
    input(&mut first, b"\x02\x0fd");
    eventually(|| server.status().is_some_and(|s| s.starts_with("2\t2\t0\t")));
    let mut reattached = server.attach();
    input(&mut reattached, b"touch reattached\n");
    eventually(|| server.root.join("reattached").exists());
}

#[test]
fn status_clicks_execute_bindings_once_and_work_with_overlays() {
    use std::sync::{Arc, Mutex};
    let server = Server::new("autosave_interval_seconds = 0\n");
    let screen = Arc::new(Mutex::new(vt100::Parser::new(24, 80, 0)));
    let parsed = screen.clone();
    let mut client = UnixStream::connect(&server.socket).unwrap();
    client.write_all(&[b'R', 0, 80, 0, 24, 0, 0, 0, 0]).unwrap();
    let mut reader = client.try_clone().unwrap();
    let reader_thread = thread::spawn(move || {
        let mut bytes = [0; 8192];
        loop {
            match reader.read(&mut bytes) {
                Ok(0) | Err(_) => break,
                Ok(count) => parsed.lock().unwrap().process(&bytes[..count]),
            }
        }
    });
    let bottom = || screen.lock().unwrap().screen().rows(0, 80).last().unwrap();
    let column = |label: &str| {
        let row = bottom();
        row.find(label)
            .map(|offset| unicode_width::UnicodeWidthStr::width(&row[..offset]) as u16 + 1)
    };
    let click = |client: &mut UnixStream, label: &str| {
        eventually(|| column(label).is_some());
        let column = column(label).unwrap();
        input(
            client,
            format!("\x1b[<0;{column};24M\x1b[<0;{column};24m").as_bytes(),
        );
    };
    click(&mut client, "UNLOCK");
    click(&mut client, "NEW WINDOW");
    eventually(|| server.status().is_some_and(|s| s.starts_with("2\t2\t1\t")));
    eventually(|| bottom().contains("LOCKED"));
    click(&mut client, "UNLOCK");
    click(&mut client, "MORE");
    eventually(|| screen.lock().unwrap().screen().contents().contains("HELP"));
    // Clicking a bottom hint while Help is open executes it and closes Help.
    click(&mut client, "NEW WINDOW");
    eventually(|| server.status().is_some_and(|s| s.starts_with("3\t3\t1\t")));
    input(&mut client, b"\x02\x17");
    eventually(|| bottom().contains("SESSION MANAGER"));
    click(&mut client, "CLOSE");
    eventually(|| bottom().contains("LOCKED"));
    // Empty bottom-bar space must not enter the shell as mouse input.
    input(&mut client, b"\x1b[<0;80;24M\x1b[<32;20;10M\x1b[<0;20;10m");
    input(&mut client, b"touch status-click-ok\n");
    eventually(|| server.root.join("status-click-ok").exists());
    assert!(server.status().unwrap().starts_with("3\t3\t1\t"));
    client.shutdown(std::net::Shutdown::Both).unwrap();
    reader_thread.join().unwrap();
}

#[test]
fn saved_scrollback_survives_restart_and_can_be_disabled_by_reload() {
    let mut server = Server::new("save_scrollback = true\nscrollback_lines = 50\n");
    let run = |server: &Server, args: &[&str]| {
        let output = server.cli(args);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    };
    run(
        &server,
        &[
            "send-keys",
            "-s",
            "work",
            "-p",
            "1",
            "--literal",
            "--enter",
            "printf 'persisted-%s\\n' history",
        ],
    );
    eventually(|| {
        run(
            &server,
            &["capture-pane", "-s", "work", "-p", "1", "--history"],
        )
        .contains("persisted-history")
    });
    run(&server, &["save-session", "-s", "work"]);
    let saved = fs::read_to_string(server.snapshot()).unwrap();
    assert!(saved.contains("persisted-history"));
    server.stop(false);
    server.start();
    let history = run(
        &server,
        &["capture-pane", "-s", "work", "-p", "1", "--history"],
    );
    assert!(history.contains("persisted-history"));
    assert!(
        !run(&server, &["capture-pane", "-s", "work", "-p", "1"]).contains("persisted-history")
    );
    fs::write(
        server.root.join("config/rustmux/config.toml"),
        "save_scrollback = false\n",
    )
    .unwrap();
    eventually(|| {
        run(&server, &["save-session", "-s", "work"]);
        !fs::read_to_string(server.snapshot())
            .unwrap()
            .contains("scrollback")
    });
    server.stop(false);
    server.start();
    assert!(
        !run(
            &server,
            &["capture-pane", "-s", "work", "-p", "1", "--history"]
        )
        .contains("persisted-history")
    );
}

#[test]
fn autosave_records_new_output_and_restores_floating_history() {
    let mut server = Server::new(
        "save_scrollback = true\nsave_scrollback_colors = true\nautosave_interval_seconds = 1\n",
    );
    let mut client = server.attach();
    input(&mut client, b"\x02i");
    input(
        &mut client,
        b"printf '\\033[32mfloating-%s\\033[0m\\n' history\n",
    );
    eventually(|| {
        fs::read_to_string(server.snapshot()).is_ok_and(|s| s.contains("floating-history"))
    });
    input(&mut client, b"printf 'later-%s\\n' output\n");
    eventually(|| fs::read_to_string(server.snapshot()).is_ok_and(|s| s.contains("later-output")));
    server.stop(false);
    server.start();
    let output = server.cli(&["capture-pane", "-s", "work", "--history"]);
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("floating-history"), "{text}");
    assert!(text.contains("later-output"), "{text}");
    assert!(server.cli(&["save-session", "-s", "work"]).status.success());
    let saved: toml::Value =
        toml::from_str(&fs::read_to_string(server.snapshot()).unwrap()).unwrap();
    assert_eq!(
        saved["floating"]["scrollback_format"].as_str(),
        Some("ansi")
    );
    assert!(
        saved["floating"]["scrollback"]
            .as_array()
            .unwrap()
            .iter()
            .any(|line| {
                let line = line.as_str().unwrap();
                line.contains("floating-history") && line.contains("\x1b[38;5;2m")
            })
    );
}

// Submit a save without waiting for its response, then use a status request as
// an event-loop barrier. This exercises shutdown/rename with a queued save.
fn queue_save(server: &Server) -> UnixStream {
    let mut stream = UnixStream::connect(&server.socket).unwrap();
    let request = b"action = 'save-session'\nsession = 'work'\n";
    stream.write_all(b"C").unwrap();
    stream
        .write_all(&(request.len() as u32).to_be_bytes())
        .unwrap();
    stream.write_all(request).unwrap();
    assert!(server.status().is_some());
    stream
}

#[test]
fn outstanding_manual_save_is_flushed_on_shutdown_and_cannot_recreate_deleted_snapshot() {
    for delete in [false, true] {
        let mut server = Server::new("save_scrollback = true\n");
        let _save = queue_save(&server);
        server.stop(delete);
        assert_eq!(server.snapshot().exists(), !delete);
        if !delete {
            let snapshot: toml::Value =
                toml::from_str(&fs::read_to_string(server.snapshot()).unwrap()).unwrap();
            assert_eq!(snapshot["tabs"].as_array().unwrap().len(), 1);
        }
    }
}

#[test]
fn queued_save_is_finished_before_renaming_a_session() {
    let mut server = Server::new("save_scrollback = true\n");
    let _save = queue_save(&server);
    let mut stream = UnixStream::connect(&server.socket).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    stream.write_all(b"N\x07renamed").unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).unwrap();
    assert_eq!(response, b"OK");
    server.socket.set_file_name("renamed.sock");
    server.stop(false);
    assert!(!server.snapshot().exists());
    assert!(
        server
            .root
            .join("state/rustmux/sessions/renamed.toml")
            .exists()
    );
}

#[test]
fn colored_scrollback_round_trips_through_restart_and_can_be_disabled() {
    let mut server = Server::new("save_scrollback = true\nsave_scrollback_colors = true\n");
    let run = |server: &Server, args: &[&str]| {
        let output = server.cli(args);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    };
    run(
        &server,
        &[
            "send-keys",
            "-s",
            "work",
            "-p",
            "1",
            "--literal",
            "--enter",
            "printf '\\033[1;38;2;12;34;56;48;5;123mcolored-%s\\033[0m\\n' history",
        ],
    );
    eventually(|| {
        run(
            &server,
            &["capture-pane", "-s", "work", "-p", "1", "--history"],
        )
        .contains("colored-history")
    });
    run(&server, &["save-session", "-s", "work"]);
    let read_pane = |server: &Server| {
        let snapshot: toml::Value =
            toml::from_str(&fs::read_to_string(server.snapshot()).unwrap()).unwrap();
        snapshot["tabs"][0]["panes"][0].clone()
    };
    let saved = read_pane(&server);
    assert_eq!(saved["scrollback_format"].as_str(), Some("ansi"));
    let colored_line = saved["scrollback"]
        .as_array()
        .unwrap()
        .iter()
        .find_map(|line| {
            line.as_str()
                .filter(|line| line.contains("colored-history"))
        })
        .unwrap()
        .to_owned();
    assert!(colored_line.contains("\x1b[38;2;12;34;56m"));
    assert!(colored_line.contains("\x1b[48;5;123m"));
    server.stop(false);
    server.start();
    run(&server, &["save-session", "-s", "work"]);
    assert!(
        read_pane(&server)["scrollback"]
            .as_array()
            .unwrap()
            .iter()
            .any(|line| line.as_str() == Some(&colored_line))
    );
    fs::write(
        server.root.join("config/rustmux/config.toml"),
        "save_scrollback = true\nsave_scrollback_colors = false\n",
    )
    .unwrap();
    eventually(|| {
        run(&server, &["save-session", "-s", "work"]);
        read_pane(&server).get("scrollback_format").is_none()
    });
    let plain = read_pane(&server);
    assert!(plain["scrollback"].as_array().unwrap().iter().any(|line| {
        line.as_str()
            .is_some_and(|line| line.contains("colored-history"))
    }));
    assert!(
        plain["scrollback"]
            .as_array()
            .unwrap()
            .iter()
            .all(|line| !line.as_str().unwrap().contains('\x1b'))
    );
}

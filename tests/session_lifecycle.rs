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
        let socket = root.join("work.sock");
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

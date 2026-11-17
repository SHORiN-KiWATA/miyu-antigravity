//! 扮 agy 的一头（起 `mcp` 子进程）和扮扩展的一头（套接字服务端）。

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use miyu_antigravity::bridge::{self, Handler, Reply, Server, Tool};
use serde_json::{Value, json};

/// 等一行至多多久。
pub const WAIT: Duration = Duration::from_secs(30);

pub const TOKEN: &str = "secret-token";

/// 扮 agy：起着的 `mcp` 子进程。丢掉时杀掉它。
pub struct Agy {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
}

impl Agy {
    /// 起 `mcp`；`bridge` 是 `Some((套接字路径, 口令))` 就设上两个环境变量，`None` 一个都不设。
    pub fn start(bridge: Option<(&Path, &str)>) -> Agy {
        Agy::start_with_home(bridge, None)
    }

    /// 同 [`Agy::start`]；`home` 是 `Some` 的把环境变量 `HOME` 设成它（可以是空的、相对的），`None` 照测试进程自己的。
    pub fn start_with_home(bridge: Option<(&Path, &str)>, home: Option<&str>) -> Agy {
        let mut command = Command::new(env!("CARGO_BIN_EXE_miyu-antigravity"));
        command
            .arg("mcp")
            .env_remove(bridge::ENV_BRIDGE)
            .env_remove(bridge::ENV_TOKEN)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        if let Some(home) = home {
            command.env("HOME", home);
        }
        if let Some((path, token)) = bridge {
            command
                .env(bridge::ENV_BRIDGE, path)
                .env(bridge::ENV_TOKEN, token);
        }
        let mut child = command.spawn().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (sender, lines) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { return };
                if sender.send(line).is_err() {
                    return;
                }
            }
        });
        let stdin = child.stdin.take();
        Agy {
            child,
            stdin,
            lines,
        }
    }

    /// 写一行原文。
    pub fn send_line(&mut self, line: &str) {
        let stdin = self.stdin.as_mut().unwrap();
        writeln!(stdin, "{line}").unwrap();
        stdin.flush().unwrap();
    }

    /// 写一条消息。
    pub fn send(&mut self, message: &Value) {
        self.send_line(&message.to_string());
    }

    /// 发一条请求。
    pub fn request(&mut self, id: i64, method: &str, params: &Value) {
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
    }

    /// 发一条通知。
    pub fn notify(&mut self, method: &str) {
        self.send(&json!({"jsonrpc": "2.0", "method": method}));
    }

    /// 读下一条回话。
    pub fn recv(&self) -> Value {
        match self.lines.recv_timeout(WAIT) {
            Ok(line) => serde_json::from_str(&line).unwrap(),
            Err(RecvTimeoutError::Timeout) => panic!("{} 秒没等来一行", WAIT.as_secs()),
            Err(RecvTimeoutError::Disconnected) => panic!("MCP 服务的标准输出关了"),
        }
    }

    /// 至少等 `quiet` 这么久，期间没有任何输出（通知不回话）。
    pub fn assert_silent(&self, quiet: Duration) {
        if let Ok(line) = self.lines.recv_timeout(quiet) {
            panic!("不该有输出，收到了 {line}");
        }
    }

    /// 发请求、读回话、核对编号。
    pub fn ask(&mut self, id: i64, method: &str, params: &Value) -> Value {
        self.request(id, method, params);
        let reply = self.recv();
        assert_eq!(reply["id"], id, "{reply}");
        assert_eq!(reply["jsonrpc"], "2.0");
        reply
    }

    /// 关标准输入。
    pub fn close_stdin(&mut self) {
        self.stdin = None;
    }

    /// 等进程退出。
    pub fn wait_exit(&mut self) -> ExitStatus {
        let deadline = Instant::now() + WAIT;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status;
            }
            assert!(Instant::now() < deadline, "进程没退出");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Agy {
    fn drop(&mut self) {
        // 已经退了的 kill 会报错，不管；然后收尸，不留进程。
        self.child.kill().ok();
        self.child.wait().ok();
    }
}

/// 一次交到扩展手里的 `call`。
pub struct Call {
    pub name: String,
    pub arguments: Value,
    pub reply: Reply,
}

/// 扮扩展的处理者：口令对的才答，`call` 交给测试。
pub struct Extension {
    pub tools: Vec<Tool>,
    pub calls: Mutex<Sender<Call>>,
    /// 请求里报来的家目录（每次一条）。
    pub homes: Arc<Mutex<Vec<PathBuf>>>,
}

impl Handler for Extension {
    fn home(&self, token: &str, home: &Path) {
        if token == TOKEN {
            self.homes.lock().unwrap().push(home.to_path_buf());
        }
    }

    fn list(&self, token: &str) -> Option<Vec<Tool>> {
        (token == TOKEN).then(|| self.tools.clone())
    }

    fn call(&self, token: &str, name: &str, arguments: Value, reply: Reply) {
        if token != TOKEN {
            reply.refuse("unknown token").unwrap();
            return;
        }
        let call = Call {
            name: name.into(),
            arguments,
            reply,
        };
        self.calls.lock().unwrap().send(call).unwrap();
    }
}

/// 扮扩展：套接字服务端和收 `call` 的一头。
pub struct Rig {
    pub server: Option<Server>,
    pub calls: Receiver<Call>,
    pub dir: PathBuf,
    pub path: PathBuf,
    /// 扩展收到的 `home`（照先后）。
    pub homes: Arc<Mutex<Vec<PathBuf>>>,
}

pub fn tool(name: &str) -> Tool {
    Tool {
        name: name.into(),
        description: format!("does {name}"),
        input_schema: json!({"type": "object", "properties": {"x": {"type": "string"}}}),
    }
}

impl Rig {
    pub fn new(name: &str) -> Rig {
        // Unix 套接字路径有长度上限：用短的系统临时目录。
        let dir = std::env::temp_dir().join(format!("ag-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.sock");
        let (sender, calls) = channel();
        let homes = Arc::new(Mutex::new(Vec::new()));
        let handler = Arc::new(Extension {
            tools: vec![tool("bash"), tool("read")],
            calls: Mutex::new(sender),
            homes: Arc::clone(&homes),
        });
        let server = Server::start(&path, handler).unwrap();
        Rig {
            server: Some(server),
            calls,
            dir,
            path,
            homes,
        }
    }

    /// 同 [`Rig::agy`]，`mcp` 的 `HOME` 是 `home`。
    pub fn agy_with_home(&self, home: &str) -> Agy {
        Agy::start_with_home(Some((&self.path, TOKEN)), Some(home))
    }

    /// 起一个带着这个套接字和口令的 `mcp`。
    pub fn agy(&self) -> Agy {
        Agy::start(Some((&self.path, TOKEN)))
    }

    /// 等扩展收到下一个 `call`。
    pub fn next_call(&self) -> Call {
        self.calls.recv_timeout(WAIT).unwrap()
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        self.server = None;
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

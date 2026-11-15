//! 集成测试扮核心：起 `miyu-antigravity serve`，经管道读写一行一条的 JSON-RPC。`antigravity.binary` 指到假 agy
//! （`tests/fake_agy.py` 的一种演法）；起扩展时把它的 `HOME` 设成临时目录，agy 照它跑，真的 `~/.gemini` 碰不到。临时
//! 目录都在 `CARGO_TARGET_TMPDIR` 下。

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// 等一行至多多久。
pub const WAIT: Duration = Duration::from_secs(30);

/// 一个扮核心的测试：起着的扩展和它的几个目录。用完（丢掉）时请它退出、删掉它的临时目录。
pub struct Core {
    child: Child,
    root: PathBuf,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
    /// 扩展的 `HOME`：agy 也照它跑。
    pub home: PathBuf,
    /// 扩展的 `MIYU_STATE_DIR`。
    pub state: PathBuf,
    /// 扩展的 `MIYU_RUNTIME_DIR`：给 MCP 服务用的套接字放在这里。Unix 套接字路径有长度上限，所以放在短的系统临时目录下，不在
    /// 很深的 target 目录里。
    pub runtime: PathBuf,
    /// 假 agy 写记录的目录。
    pub record: PathBuf,
    /// 假 agy 的包装脚本。
    pub binary: PathBuf,
}

/// 一次调用收到的：增量（照先后）、每条 `driver.delta` 原来那一行有多长、回应。
pub struct Collected {
    pub deltas: Vec<Value>,
    pub line_lengths: Vec<usize>,
    pub response: Value,
}

impl Collected {
    /// 正文块的字接起来。
    pub fn text(&self) -> String {
        self.deltas
            .iter()
            .filter_map(|delta| delta.pointer("/text/text").and_then(Value::as_str))
            .collect()
    }
}

/// 给测试新建一个空目录。
pub fn scratch(name: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "{name}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).unwrap();
    }
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// 一份最小的统一请求：系统提示词 `system`，人说一句 `text`。
pub fn request(system: &str, text: &str) -> Value {
    json!({
        "tools": [],
        "system": system,
        "messages": [{"role": "user", "blocks": [{"type": "text", "text": text}]}],
        "stable": 0
    })
}

/// 一段内容的指纹：SHA-256 的前 16 位十六进制（核心的写法，见扩展参考 `driver.call` 的 `fingerprint`）。
pub fn hash(value: &Value) -> String {
    let digest = Sha256::digest(value.to_string().as_bytes());
    hex(&digest[..8])
}

/// 字节写成小写十六进制。
pub fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .flat_map(|byte| [byte >> 4, byte & 0x0f])
        .map(|nibble| char::from_digit(u32::from(nibble), 16).expect("a nibble is a hex digit"))
        .collect()
}

/// 请求的 `fingerprint`：工具面、system、每条消息各一个。
pub fn fingerprint(request: &Value) -> Value {
    let messages: Vec<String> = request["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(hash)
        .collect();
    json!({"tools": hash(&request["tools"]), "system": hash(&request["system"]), "messages": messages})
}

/// 在 `request` 后面接上一条 assistant（`answer`）和几条 user（`texts`）：同一段对话的下一轮。
pub fn next_turn(request: &Value, answer: &str, texts: &[&str]) -> Value {
    let mut next = request.clone();
    let messages = next["messages"].as_array_mut().unwrap();
    messages.push(json!({"role": "assistant", "blocks": [{"type": "text", "text": answer}]}));
    for text in texts {
        messages.push(json!({"role": "user", "blocks": [{"type": "text", "text": text}]}));
    }
    next
}

/// 进程 `pid` 还在（活着或者还没被收尸）。
pub fn alive(pid: i64) -> bool {
    let pid = nix::unistd::Pid::from_raw(i32::try_from(pid).unwrap());
    nix::sys::signal::kill(pid, None).is_ok()
}

/// 等到以 `pid` 为组号的进程组整组没了。
pub fn wait_group_gone(pid: i64) {
    let group = nix::unistd::Pid::from_raw(i32::try_from(pid).unwrap());
    let deadline = Instant::now() + WAIT;
    while nix::sys::signal::killpg(group, None).is_ok() {
        assert!(Instant::now() < deadline, "进程组 {group} 还在");
        std::thread::sleep(Duration::from_millis(20));
    }
}

impl Core {
    /// 起扩展、握手：假 agy 照 `mode` 演。
    pub fn start(name: &str, mode: &str) -> Core {
        Core::start_with(name, mode, &[], &json!({}))
    }

    /// 同 [`Core::start`]，扩展另带环境变量 `env`，握手的配置另加 `config` 里的键。
    pub fn start_with(name: &str, mode: &str, env: &[(&str, &str)], config: &Value) -> Core {
        let mut core = Core::spawn_with(name, mode, env);
        let mut keys = json!({"antigravity.binary": core.binary.to_str().unwrap()});
        for (key, value) in config.as_object().unwrap() {
            keys[key] = value.clone();
        }
        let config = keys;
        core.answer_hello(
            &json!({"protocol": 1, "language": "en", "config": config}),
            None,
        );
        core
    }

    /// 只起不握手。
    pub fn spawn(name: &str, mode: &str) -> Core {
        Core::spawn_with(name, mode, &[])
    }

    /// 只起不握手，扩展另带环境变量 `env`。
    pub fn spawn_with(name: &str, mode: &str, env: &[(&str, &str)]) -> Core {
        let root = scratch(name);
        let [home, state, record, bin] = ["home", "state", "record", "bin"].map(|part| {
            let dir = root.join(part);
            std::fs::create_dir_all(&dir).unwrap();
            dir
        });
        let runtime = std::env::temp_dir().join(format!(
            "ag-{}-{}",
            std::process::id(),
            root.file_name().unwrap().to_string_lossy()
        ));
        std::fs::create_dir_all(&runtime).unwrap();
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let binary = bin.join("agy");
        std::fs::write(
            &binary,
            format!(
                "#!/bin/sh\nexec python3 '{}' '{mode}' \"$@\"\n",
                manifest.join("tests/fake_agy.py").display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();

        let mut command = Command::new(env!("CARGO_BIN_EXE_miyu-antigravity"));
        command.env("MIYU_RUNTIME_DIR", &runtime);
        command.envs(env.iter().copied());
        let mut child = command
            .arg("serve")
            .env("HOME", &home)
            .env("MIYU_STATE_DIR", &state)
            .env("FAKE_RECORD", &record)
            .env("FAKE_SAMPLES", manifest.join("tests/samples"))
            .current_dir(&state)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
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
        Core {
            child,
            root,
            stdin,
            lines,
            home,
            state,
            runtime,
            record,
            binary,
        }
    }

    /// 读握手、回答：`result` 或 `error` 二选一。
    pub fn answer_hello(&mut self, result: &Value, error: Option<&Value>) {
        let hello = self.recv();
        assert_eq!(hello["method"], "hello", "{hello}");
        assert_eq!(hello["params"]["protocol"], json!([1, 1]));
        assert!(hello["params"].get("token").is_none(), "扩展不出示凭据");
        let id = hello["id"].clone();
        match error {
            Some(error) => self.send(&json!({"jsonrpc": "2.0", "id": id, "error": error})),
            None => self.send(&json!({"jsonrpc": "2.0", "id": id, "result": result})),
        }
    }

    /// 写一条。
    pub fn send(&mut self, message: &Value) {
        let stdin = self.stdin.as_mut().unwrap();
        writeln!(stdin, "{message}").unwrap();
        stdin.flush().unwrap();
    }

    /// 读下一条的原文。
    pub fn recv_line(&self) -> String {
        match self.lines.recv_timeout(WAIT) {
            Ok(line) => line,
            Err(RecvTimeoutError::Timeout) => panic!("{} 秒没等来一行", WAIT.as_secs()),
            Err(RecvTimeoutError::Disconnected) => panic!("扩展的标准输出关了"),
        }
    }

    /// 读下一条。
    pub fn recv(&self) -> Value {
        serde_json::from_str(&self.recv_line()).unwrap()
    }

    /// 发一次 `driver.call`：请求编号 `id`、调用编号 `call`、模型 `gemini-3.8-flash-low`，指纹照请求算（[`fingerprint`]）。
    pub fn call(&mut self, id: &str, call: &str, request: &Value) {
        self.call_model(id, call, request, "gemini-3.8-flash-low");
    }

    /// 同 [`Core::call`]，模型是 `model`。
    pub fn call_model(&mut self, id: &str, call: &str, request: &Value, model: &str) {
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "driver.call",
            "params": {
                "call": call,
                "driver": "antigravity/antigravity",
                "request": request,
                "fingerprint": fingerprint(request),
                "params": {"model": model}
            }
        }));
    }

    /// 一直读到这几个请求都回应了，照调用编号分好增量。`calls` 是 `(请求编号, 调用编号)`。另交收到的先后：每条写成
    /// `delta:<调用编号>` 或 `answer:<调用编号>`。
    pub fn collect_all(&self, calls: &[(&str, &str)]) -> (HashMap<String, Collected>, Vec<String>) {
        let mut got: HashMap<String, Collected> = calls
            .iter()
            .map(|(_, call)| {
                (
                    (*call).to_string(),
                    Collected {
                        deltas: Vec::new(),
                        line_lengths: Vec::new(),
                        response: Value::Null,
                    },
                )
            })
            .collect();
        let mut order = Vec::new();
        let mut waiting = calls.len();
        while waiting > 0 {
            let line = self.recv_line();
            let message: Value = serde_json::from_str(&line).unwrap();
            if message["method"] == "driver.delta" {
                let call = message["params"]["call"].as_str().unwrap();
                let slot = got
                    .get_mut(call)
                    .unwrap_or_else(|| panic!("不认识的调用 {call}"));
                assert!(slot.response.is_null(), "回应以后又来了增量");
                slot.deltas.extend(
                    message["params"]["deltas"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .cloned(),
                );
                slot.line_lengths.push(line.len());
                order.push(format!("delta:{call}"));
            } else if let Some((_, call)) = calls.iter().find(|(id, _)| message["id"] == *id) {
                let slot = got.get_mut(*call).unwrap();
                assert!(slot.response.is_null(), "回应了两次");
                slot.response = message;
                order.push(format!("answer:{call}"));
                waiting -= 1;
            } else {
                panic!("没想到的一条：{line}");
            }
        }
        (got, order)
    }

    /// 一次调用：读到回应为止。
    pub fn collect(&self, id: &str, call: &str) -> Collected {
        self.collect_all(&[(id, call)]).0.remove(call).unwrap()
    }

    /// 假 agy 写的全部记录。
    pub fn records(&self) -> Vec<Value> {
        std::fs::read_dir(&self.record)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
            .map(|path| serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap())
            .collect()
    }

    /// 进程 `pid` 那个假 agy 的记录。
    pub fn record_of(&self, pid: i64) -> Value {
        self.records()
            .into_iter()
            .find(|record| record["pid"] == pid)
            .unwrap_or_else(|| panic!("没有进程 {pid} 的记录"))
    }

    /// 等到有一份记录带着 `key`。
    pub fn wait_record(&self, key: &str) -> Value {
        let deadline = Instant::now() + WAIT;
        loop {
            if let Some(record) = self
                .records()
                .into_iter()
                .find(|record| record.get(key).is_some())
            {
                return record;
            }
            assert!(Instant::now() < deadline, "假 agy 一直没写 {key}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// 工作目录的父目录里还剩几个。
    pub fn work_dirs_left(&self) -> usize {
        std::fs::read_dir(self.state.join("work")).map_or(0, Iterator::count)
    }

    /// 关标准输入（请退出），等它退出。
    pub fn finish(&mut self) -> ExitStatus {
        drop(self.stdin.take());
        self.wait_exit()
    }

    /// 等它自己退出。
    pub fn wait_exit(&mut self) -> ExitStatus {
        let deadline = Instant::now() + WAIT;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status;
            }
            assert!(Instant::now() < deadline, "扩展没退出");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Core {
    /// 关标准输入请它退出（它会收掉留着的 agy），等不到的杀掉；再删临时目录。
    fn drop(&mut self) {
        drop(self.stdin.take());
        let deadline = Instant::now() + WAIT;
        while self.child.try_wait().ok().flatten().is_none() {
            if Instant::now() >= deadline {
                self.child.kill().ok();
                self.child.wait().ok();
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        std::fs::remove_dir_all(&self.root).ok();
        std::fs::remove_dir_all(&self.runtime).ok();
    }
}

/// 等进程 `pid` 没了（被收尸、不在了）。
pub fn wait_gone(pid: i64) {
    let pid = nix::unistd::Pid::from_raw(i32::try_from(pid).unwrap());
    let deadline = Instant::now() + WAIT;
    while nix::sys::signal::kill(pid, None).is_ok() {
        assert!(Instant::now() < deadline, "进程 {pid} 还在");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// agy 那边这段对话的文件都没了。进程没了以后扩展才删文件（`Live` 收尾时先收进程、后删文件），所以等到没了为止，
/// 到点还在的才算错。
pub fn assert_conversation_removed(home: &Path, conversation: &str) {
    let base = home.join(".gemini/antigravity-cli");
    let mut paths: Vec<_> = [".db", ".db-shm", ".db-wal"]
        .iter()
        .map(|suffix| {
            base.join("conversations")
                .join(format!("{conversation}{suffix}"))
        })
        .collect();
    paths.push(base.join("brain").join(conversation));
    let deadline = Instant::now() + WAIT;
    loop {
        let left: Vec<_> = paths.iter().filter(|path| path.exists()).collect();
        if left.is_empty() {
            return;
        }
        assert!(Instant::now() < deadline, "还在：{left:?}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

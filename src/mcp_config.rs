//! agy 的全局 MCP 登记 `<HOME>/.gemini/config/mcp_config.json` 里本包那一条 `mcpServers.miyu` 的改写（设计第六节「工具面」、
//! 第十一节第 2 条）。agy 只认这一处，主人自己也可能往里写别的服务：**只改 `miyu` 这一条**，别的条目、别的顶层格一个字段
//! 都不丢，键的先后照旧（`serde_json` 开着 `preserve_order`）。
//!
//! 那一条写成 `{"command": <程序>, "args": ["mcp"], "timeoutSeconds": N, "tools": {<名>: {"eager": true}, ..}}`：`tools` 是
//! 这条里原有的工具名并上这次要登记的（名单只增不减：别的 agy 进程可能还在用旧名单；扩展正常退出时整条拿掉，名单跟着清零）；**不写 `env`**（口令、套接字地址由
//! agy 的环境变量传下来，设计第三节），旧版 Miyu 留下的同名条目连它的 `env` 一起被接管改写。
//!
//! 纯的部分（[`register_text`]、[`unregister_text`]：旧文本进、新文本或「不变」出）和读写文件（[`register`]、[`unregister`]）
//! 分开。文件读不成 JSON 对象、`mcpServers` 不是对象的，**不覆盖**，出错里写明是哪个文件。内容没变不写；要写的先写同目录的
//! 暂存文件、同步到盘再改名，agy 读到的总是整份。几次调用同时来，靠进程内的一把锁排队。

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};

use serde_json::{Map, Value, json};

use crate::workspace;

/// 本包在 `mcpServers` 里的条目名（agy 那边的工具名是 `mcp_miyu_<名>`）。
pub const SERVER: &str = "miyu";

/// 要登记的内容。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registration {
    /// 本程序的绝对路径（agy 拉起 MCP 服务时执行它，带参数 `mcp`）。
    pub program: PathBuf,
    /// agy 等一个 MCP 调用的秒数（`timeoutSeconds`）。
    pub timeout_seconds: u64,
    /// 要直接开放（`eager`）的工具名，并进原有的。
    pub tools: Vec<String>,
}

/// 同一个进程里几个调用同时改同一份文件时排队。
static LOCK: Mutex<()> = Mutex::new(());

/// 这个进程登记成过没有（[`register`] 成了就记下）：退出时只有登记过的才去收（[`registered`]）。
static REGISTERED: AtomicBool = AtomicBool::new(false);

/// `home` 下 agy 的 MCP 登记文件。
#[must_use]
pub fn path(home: &Path) -> PathBuf {
    home.join(".gemini/config/mcp_config.json")
}

/// 在 `old`（文件原文，`None` 是文件不存在）上登记 `registration`：交新文本，内容没变交 `None`。
///
/// # Errors
///
/// `old` 不是 JSON、不是对象、`mcpServers` 不是对象：说是哪一种（不带路径，由 [`register`] 补）。
pub fn register_text(
    old: Option<&str>,
    registration: &Registration,
) -> Result<Option<String>, String> {
    let before = parse(old)?;
    let mut after = before.clone();
    let servers = servers_of(&mut after)?;
    let mut tools = Map::new();
    if let Some(Value::Object(existing)) = servers.get(SERVER).and_then(|entry| entry.get("tools"))
    {
        for name in existing.keys() {
            tools.insert(name.clone(), json!({"eager": true}));
        }
    }
    for name in &registration.tools {
        tools.insert(name.clone(), json!({"eager": true}));
    }
    servers.insert(
        SERVER.into(),
        json!({
            "command": registration.program.to_string_lossy(),
            "args": ["mcp"],
            "timeoutSeconds": registration.timeout_seconds,
            "tools": tools,
        }),
    );
    Ok(rendered(&before, &after))
}

/// 在 `old` 上拿掉 `miyu` 一条：交新文本，本来就没有（含文件不存在）交 `None`。
///
/// # Errors
///
/// 同 [`register_text`]。
pub fn unregister_text(old: Option<&str>) -> Result<Option<String>, String> {
    let before = parse(old)?;
    let mut after = before.clone();
    // 没有 `mcpServers` 的文件不用建出来；有的才看里面。
    if after.get("mcpServers").is_some() {
        servers_of(&mut after)?.shift_remove(SERVER);
    }
    Ok(rendered(&before, &after))
}

/// 在 `old` 上拿掉 `miyu` 一条，**只在它的 `command` 正好是 `program` 时**：交新文本；条目没有、指着别的程序（另一处装的 Miyu
/// 登记的）的交 `None`。扩展退出时用（[`unregister_own`]）。
///
/// # Errors
///
/// 同 [`register_text`]。
pub fn unregister_own_text(old: Option<&str>, program: &Path) -> Result<Option<String>, String> {
    let before = parse(old)?;
    let mut after = before.clone();
    if after.get("mcpServers").is_some() {
        let servers = servers_of(&mut after)?;
        let ours = servers
            .get(SERVER)
            .and_then(|entry| entry.get("command"))
            .and_then(Value::as_str)
            .is_some_and(|command| Path::new(command) == program);
        if ours {
            servers.shift_remove(SERVER);
        }
    }
    Ok(rendered(&before, &after))
}

/// 读成 JSON 对象；文件不存在、文件是空的（只有空白也算）都是空对象：agy 头一次运行会自己建一份 0 字节的
/// `mcp_config.json`（agy 1.4.0 真机见过），没配过 MCP 的人手里就是这样一份，里面没有要保住的东西。
fn parse(old: Option<&str>) -> Result<Value, String> {
    let Some(text) = old.filter(|text| !text.trim().is_empty()) else {
        return Ok(json!({}));
    };
    match serde_json::from_str::<Value>(text) {
        Ok(value @ Value::Object(_)) => Ok(value),
        Ok(_) => Err("it is not a JSON object".into()),
        Err(error) => Err(format!("it is not valid JSON ({error})")),
    }
}

/// `root`（已知是对象）里的 `mcpServers`，没有就建。
fn servers_of(root: &mut Value) -> Result<&mut Map<String, Value>, String> {
    let Some(map) = root.as_object_mut() else {
        return Err("it is not a JSON object".into());
    };
    map.entry("mcpServers")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| "\"mcpServers\" is not a JSON object".to_string())
}

/// 值变了才出文本（带缩进、末尾换行）；没变是 `None`。
fn rendered(before: &Value, after: &Value) -> Option<String> {
    if before == after {
        return None;
    }
    let mut text = serde_json::to_string_pretty(after).ok()?;
    text.push('\n');
    Some(text)
}

/// 把 `registration` 登记进 `file`：没有就建出来，内容没变不写。交的是有没有写。
///
/// # Errors
///
/// 文件读不出来、读不懂（不是 JSON 对象、`mcpServers` 不是对象；**不覆盖**，错里有路径）、写不进去：原样交。
pub fn register(file: &Path, registration: &Registration) -> std::io::Result<bool> {
    let wrote = update(file, |old| register_text(old, registration))?;
    REGISTERED.store(true, Ordering::SeqCst);
    Ok(wrote)
}

/// 把 `file` 里的 `miyu` 一条拿掉：文件不存在、条目本来就没有算没写。交的是有没有写。
///
/// # Errors
///
/// 同 [`register`]。
pub fn unregister(file: &Path) -> std::io::Result<bool> {
    update(file, unregister_text)
}

/// 扩展退出时把自己登记的那一条收掉：`file` 里的 `miyu` 一条还指着 `program`（本程序）的才拿掉，别的不动。交的是有没有写。
/// 这样卸了包以后 agy 的配置里不留一条指着已经删掉的程序的服务，工具名单也不会一直攒下去；被硬杀的来不及收，留给
/// `miyu antigravity unregister` 或者下一次正常退出。
///
/// # Errors
///
/// 同 [`register`]。
pub fn unregister_own(file: &Path, program: &Path) -> std::io::Result<bool> {
    update(file, |old| unregister_own_text(old, program))
}

/// 这个进程登记成过没有。
#[must_use]
pub fn registered() -> bool {
    REGISTERED.load(Ordering::SeqCst)
}

/// 加锁读 `file`，交给 `change` 算新文本，有变化才写。
fn update(
    file: &Path,
    change: impl FnOnce(Option<&str>) -> Result<Option<String>, String>,
) -> std::io::Result<bool> {
    let _guard = LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    let old = match std::fs::read_to_string(file) {
        Ok(text) => Some(text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(with_path(file, &error)),
    };
    let new = change(old.as_deref()).map_err(|reason| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("cannot understand {}: {reason}", file.display()),
        )
    })?;
    // 文件本来不存在、又没什么要写的（拿掉条目）：什么都不建。
    let Some(text) = new else {
        return Ok(false);
    };
    write_replacing(file, &text).map_err(|error| with_path(file, &error))?;
    Ok(true)
}

fn with_path(file: &Path, error: &std::io::Error) -> std::io::Error {
    std::io::Error::new(error.kind(), format!("{}: {error}", file.display()))
}

/// 先写同目录的暂存文件、同步到盘，再改名顶替 `file`；原文件的权限带过去。
fn write_replacing(file: &Path, text: &str) -> std::io::Result<()> {
    let Some(dir) = file.parent() else {
        return Err(std::io::Error::other("the file has no parent directory"));
    };
    std::fs::create_dir_all(dir)?;
    let temporary = dir.join(format!(".mcp_config.{}.tmp", workspace::unique()));
    let written = (|| {
        let mut out = std::fs::File::create_new(&temporary)?;
        if let Ok(metadata) = std::fs::metadata(file) {
            out.set_permissions(metadata.permissions())?;
        }
        out.write_all(text.as_bytes())?;
        out.sync_all()?;
        std::fs::rename(&temporary, file)
    })();
    if written.is_err()
        && let Err(error) = std::fs::remove_file(&temporary)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        // 删不掉暂存文件只记一行，不盖住原来的错。
        eprintln!("removing {} failed: {error}", temporary.display());
    }
    written
}

#[cfg(test)]
mod tests;

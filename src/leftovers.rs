//! 扩展进程被硬杀（`SIGKILL`、断电）以后留下的东西，和下一个扩展进程启动时的清扫（设计第八节「清理」）。
//!
//! 正常收尾时 [`crate::live::Live`] 自己删：套接字服务端删套接字文件、工作目录和 agy 那边这段对话的文件都在丢掉 `Live` 时删。
//! 硬杀了没人动手，留下三样，名字里都带着扩展进程的进程号或者记着它：
//!
//! 1. 套接字目录（`MIYU_RUNTIME_DIR`，没有用 `MIYU_STATE_DIR`）下的 `antigravity-<进程号>.sock`；
//! 2. `<MIYU_STATE_DIR>/work/<进程号>-…` 工作目录（[`crate::workspace::create_work_dir`]）；
//! 3. agy 记下的对话（`conversations/<编号>.db*`、`brain/<编号>/`）：编号只有那个进程知道，所以得知编号时在
//!    `<MIYU_STATE_DIR>/kept/<编号>` 记一个小文件：头一行是扩展的进程号（十进制）；第二行（可选）是 agy 实际用的家目录——
//!    包装脚本给 agy 换了 `HOME` 的时候，会话文件在那里（[`crate::agy_home`]）。第二行是绝对路径的清扫时照它删，没有（老版本留下的
//!    记号只有一行）或者不是绝对路径的照扩展自己的 `HOME`。收尾删完会话文件以后记号也删（[`mark`]、[`mark_with_home`]、
//!    [`unmark`]）。
//!
//! 启动时清扫一次（[`sweep`]）：名字里的、记号里的进程号不是自己、而且那个进程已经不在了的，照名字删。「不在了」用
//! `kill(pid, 0)` 问：`ESRCH` 是不在，别的（包括没有权限）一律当还在。进程号被别的程序重用了的会被当成还在，这一次不清，等
//! 那个进程号再空出来的那次启动；宁可漏清，不错删。自己的进程号的东西、名字对不上这几种格式的文件，一概不碰。整件事出任何错
//! 只记日志，不挡着启动。

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use nix::errno::Errno;
use nix::sys::signal::kill;
use nix::unistd::Pid;

use crate::workspace::{self, absent_ok};

/// 记号的目录：`<state>/kept`。
#[must_use]
pub fn kept_dir(state: &Path) -> PathBuf {
    state.join("kept")
}

/// 记一下：对话 `conversation` 是本进程的 agy 在用的，会话文件在扩展自己的 `HOME` 下（只写进程号一行）。
///
/// # Errors
///
/// 同 [`mark_with_home`]。
pub fn mark(state: &Path, conversation: &str) -> std::io::Result<()> {
    mark_with_home(state, conversation, None)
}

/// 同 [`mark`]；`agy_home` 是 `Some`（agy 实际用的家目录，和扩展自己的不一样）的再写第二行。先写临时文件再改名，别的进程读到的
/// 不会是半个；已经有的记号这样重写也不会丢（补写用）。
///
/// # Errors
///
/// 编号不合 [`workspace::valid_conversation`]、写不了：原样交。
pub fn mark_with_home(
    state: &Path,
    conversation: &str,
    agy_home: Option<&Path>,
) -> std::io::Result<()> {
    if !workspace::valid_conversation(conversation) {
        return Err(std::io::Error::other(format!(
            "conversation id {conversation:?} is not letters, digits and '-'"
        )));
    }
    let dir = kept_dir(state);
    std::fs::create_dir_all(&dir)?;
    let temporary = dir.join(format!(".{conversation}.tmp"));
    let content = match agy_home {
        Some(home) => format!("{}\n{}\n", std::process::id(), home.display()),
        None => format!("{}\n", std::process::id()),
    };
    let written = std::fs::write(&temporary, content)
        .and_then(|()| std::fs::rename(&temporary, dir.join(conversation)));
    if written.is_err() {
        std::fs::remove_file(&temporary).ok();
    }
    written
}

/// 删掉对话 `conversation` 的记号；本来就没有的不算错。
///
/// # Errors
///
/// 删不掉：原样交。
pub fn unmark(state: &Path, conversation: &str) -> std::io::Result<()> {
    absent_ok(std::fs::remove_file(kept_dir(state).join(conversation)))
}

/// 启动时清扫一次：套接字目录 `runtime`（没有的跳过）、`state` 下的工作目录和记号；`home` 是扩展自己的家目录（没有的，
/// 记号不动，会话文件删不了）。出的错只记日志。
pub fn sweep(state: &Path, runtime: Option<&Path>, home: Option<&Path>) {
    let me = std::process::id();
    if let Some(runtime) = runtime {
        sweep_sockets(runtime, me);
    }
    sweep_work(&state.join("work"), me);
    match home {
        Some(home) => sweep_marks(state, home, me),
        None => eprintln!(
            "cleaning up: HOME is not set, so the conversations left by a killed extension are not removed"
        ),
    }
}

fn sweep_sockets(dir: &Path, me: u32) {
    let Some(names) = list(dir) else { return };
    for name in dead_sockets(&names, me, process_alive) {
        let path = dir.join(name);
        match absent_ok(std::fs::remove_file(&path)) {
            Ok(()) => eprintln!(
                "cleaning up: removed {}, left by an extension that is gone",
                path.display()
            ),
            Err(error) => eprintln!("cleaning up: could not remove {}: {error}", path.display()),
        }
    }
}

fn sweep_work(dir: &Path, me: u32) {
    let Some(names) = list(dir) else { return };
    for name in dead_work(&names, me, process_alive) {
        let path = dir.join(name);
        match workspace::remove_work_dir(&path) {
            Ok(()) => eprintln!(
                "cleaning up: removed {}, left by an extension that is gone",
                path.display()
            ),
            Err(error) => eprintln!("cleaning up: could not remove {}: {error}", path.display()),
        }
    }
}

fn sweep_marks(state: &Path, home: &Path, me: u32) {
    let dir = kept_dir(state);
    let Some(names) = list(&dir) else { return };
    let marks: Vec<(String, String)> = names
        .into_iter()
        .map(|name| {
            let content = read_small(&dir.join(&name));
            (name, content)
        })
        .collect();
    let contents: HashMap<&str, &str> = marks
        .iter()
        .map(|(name, content)| (name.as_str(), content.as_str()))
        .collect();
    for verdict in dead_marks(&marks, me, process_alive) {
        match verdict {
            Verdict::Conversation(id) => {
                let content = contents.get(id.as_str()).copied().unwrap_or_default();
                let at = mark_home(content);
                match workspace::remove_conversation(at.as_deref().unwrap_or(home), &id) {
                    Ok(()) => {
                        eprintln!(
                            "cleaning up: removed agy's conversation {id}, left by an extension that is gone"
                        );
                        if let Err(error) = unmark(state, &id) {
                            eprintln!("cleaning up: could not remove the mark of {id}: {error}");
                        }
                    }
                    // 记号留着，下次启动再试。
                    Err(error) => {
                        eprintln!("cleaning up: could not remove agy's conversation {id}: {error}");
                    }
                }
            }
            Verdict::Unreadable(id) => {
                eprintln!(
                    "cleaning up: the mark of conversation {id} does not hold a process id; removing the mark only, not the conversation"
                );
                if let Err(error) = unmark(state, &id) {
                    eprintln!("cleaning up: could not remove the mark of {id}: {error}");
                }
            }
        }
    }
}

/// 目录里的名字（不是 UTF-8 的不算）；目录不在不算错，别的错记一行日志。
fn list(dir: &Path) -> Option<Vec<String>> {
    match std::fs::read_dir(dir) {
        Ok(entries) => Some(
            entries
                .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
                .collect(),
        ),
        Err(error) => {
            if error.kind() != std::io::ErrorKind::NotFound {
                eprintln!("cleaning up: could not list {}: {error}", dir.display());
            }
            None
        }
    }
}

/// 读文件开头一小段（记号是一行进程号、可能再加一行路径）；读不了的当空的。
fn read_small(path: &Path) -> String {
    let mut text = String::new();
    std::fs::File::open(path)
        .and_then(|file| file.take(MARK_LIMIT).read_to_string(&mut text))
        .map_or_else(|_| String::new(), |_| text)
}

/// 记号至多读多少字节：进程号一行加一个路径（至多 4096 字节）一行，留余量。
const MARK_LIMIT: u64 = 8192;

/// 记号第二行的 agy 家目录：头一行之后的那一行、以换行收尾（没写完的不算）、是绝对路径的才算；别的（没有、相对路径）是
/// `None`，照扩展自己的 `HOME`。
fn mark_home(content: &str) -> Option<PathBuf> {
    let (_, rest) = content.split_once('\n')?;
    let (line, _) = rest.split_once('\n')?;
    Some(PathBuf::from(line)).filter(|path| path.is_absolute())
}

/// 进程 `pid` 还在不在：`ESRCH` 是不在了，别的结果（有权限的、没有权限的）都当还在。
fn process_alive(pid: u32) -> bool {
    let Ok(raw) = i32::try_from(pid) else {
        return true;
    };
    !matches!(kill(Pid::from_raw(raw), None), Err(Errno::ESRCH))
}

/// 一段十进制数写的进程号；0（对整组发信号）、超出 `i32` 的、带别的字符的都不是。
fn parse_pid(text: &str) -> Option<u32> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse::<u32>()
        .ok()
        .filter(|pid| (1..=i32::MAX.unsigned_abs()).contains(pid))
}

/// 套接字文件名里的进程号：`antigravity-<数字>.sock`。
fn pid_of_socket(name: &str) -> Option<u32> {
    parse_pid(name.strip_prefix("antigravity-")?.strip_suffix(".sock")?)
}

/// 工作目录名里的进程号：`<数字>-…`。
fn pid_of_work(name: &str) -> Option<u32> {
    parse_pid(name.split_once('-')?.0)
}

/// `names` 里该清的：名字合格式、进程号不是 `me`、而且 `alive` 说它不在了。
fn stale<'a>(
    names: &'a [String],
    me: u32,
    alive: &impl Fn(u32) -> bool,
    pid_of: fn(&str) -> Option<u32>,
) -> Vec<&'a str> {
    names
        .iter()
        .filter(|name| pid_of(name).is_some_and(|pid| pid != me && !alive(pid)))
        .map(String::as_str)
        .collect()
}

/// 套接字目录里该删的文件名。
fn dead_sockets(names: &[String], me: u32, alive: impl Fn(u32) -> bool) -> Vec<&str> {
    stale(names, me, &alive, pid_of_socket)
}

/// 工作目录的父目录里该删的目录名。
fn dead_work(names: &[String], me: u32, alive: impl Fn(u32) -> bool) -> Vec<&str> {
    stale(names, me, &alive, pid_of_work)
}

/// 对一个该清的记号怎么办。
#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    /// 记的进程没了：删这段对话（编号）的会话文件，再删记号。
    Conversation(String),
    /// 内容读不懂：只删记号，不删会话文件。
    Unreadable(String),
}

/// `marks` 是 `(记号的名字，内容)`：名字就是对话编号，不合 [`workspace::valid_conversation`] 的不碰；内容是别的进程（不是
/// `me`）的、而且 `alive` 说它不在了的，要清会话；内容读不懂的只清记号。
fn dead_marks(marks: &[(String, String)], me: u32, alive: impl Fn(u32) -> bool) -> Vec<Verdict> {
    marks
        .iter()
        .filter(|(name, _)| workspace::valid_conversation(name))
        .filter_map(|(name, content)| {
            match parse_pid(content.lines().next().unwrap_or("").trim()) {
                None => Some(Verdict::Unreadable(name.clone())),
                Some(pid) if pid != me && !alive(pid) => Some(Verdict::Conversation(name.clone())),
                Some(_) => None,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests;

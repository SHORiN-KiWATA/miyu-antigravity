//! 一个活着的 agy 进程，也就是一段 agy 对话（设计第三节「agy 进程」、第二节「进程复用」）：连着收好几轮，几次 `driver.call`
//! 之间留着。
//!
//! - 标准输入：一个写的线程，按先后写进来的每一行；agy 没读就退出的，那一行写不进去、记一行日志，线程结束，不卡调用的人。
//! - 标准输出：一个读的线程，按行读，送给「这一轮在听的那一次调用」（[`Live::route`]）；闲着时没人听，读到了记一行日志、记下
//!   「乱说话了」（[`Live::stray`]）。读到头（agy 退了）记下来（[`Live::closed`]），巡检和下一轮照它发现进程没了。
//! - 带工具的（[`Wiring`]）：起的时候把套接字地址和口令放进环境变量，总台（[`crate::hub`]）照口令找到这一个进程的路由和工具表；
//!   一轮以交回一件工具调用结束时，这件调用的 [`Reply`] 跟着 [`Live`] 挂着（[`Live::set_pending`]），等下一次请求带结果来。
//! - 收掉（丢掉这个值时）：收进程组，删工作目录和 agy 那边这段对话的文件（设计第八节「清理」）。agy 的 MCP 服务报过家目录的
//!   （包装脚本给 agy 换了 `HOME`），读转录、删会话文件都照它（[`Live::agy_home`]）。
//! - 得知对话编号时在状态目录里记一个号（[`crate::leftovers::mark_with_home`]，由 [`AgyHome`] 管），扩展被硬杀了，下一个扩展进程启动时照它清会话文件；
//!   会话文件删干净了记号才删。

use std::io::{BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;

use crate::agy_home::AgyHome;
use crate::bridge::{self, Reply};
use crate::call::Signal;
use crate::config::Config;
use crate::hub::Wiring;
use crate::process::{self, Line};
use crate::{agy, workspace};

/// 读的线程把这一行送给谁。
pub type Route = Arc<Mutex<Option<Sender<Signal>>>>;

/// 挂着的工具调用：交回核心的那件的编号，和 agy 那头（经 MCP 服务）等着回话的连接。丢掉它连接就关了。
#[derive(Debug)]
pub struct Pending {
    /// 这件调用的编号（总台给的，从 1 起）。
    pub n: u64,
    /// 等着回话的连接。
    pub reply: Reply,
}

/// 一个活着的 agy 进程。
pub struct Live {
    child: Child,
    input: Option<Sender<String>>,
    writer: Option<JoinHandle<()>>,
    route: Route,
    closed: Arc<AtomicBool>,
    /// 没人听的时候流里来了一行、或者总台收到了调用：agy 那边已经不是我们以为的样子了。
    stray: Arc<AtomicBool>,
    /// 挂着的工具调用（一轮以交回工具调用结束时有）。
    pending: Option<Pending>,
    /// 总台里登记的这个进程的口令（带工具的才有）；丢掉就从总台拿掉。
    wiring: Option<Wiring>,
    /// 这段对话用的代理名。
    agent: String,
    /// agy 这段对话的编号（`init` 里的；还不知道的是 `None`）。
    conversation: Option<String>,
    /// 给它新建的工作目录。
    work: PathBuf,
    /// 扩展自己的家目录（`HOME`）。
    home: PathBuf,
    /// agy 实际用的家目录（会话文件、转录在这里）、记号。
    agy_home: Arc<AgyHome>,
}

impl std::fmt::Debug for Live {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Live")
            .field("pid", &self.child.id())
            .field("agent", &self.agent)
            .field("conversation", &self.conversation)
            .finish_non_exhaustive()
    }
}

impl Live {
    /// 照 `config` 起一个 agy：模型 `model`、代理 `agent`、工作目录 `work`（也是 `--add-dir`），家目录 `home`（就是扩展自己的
    /// `HOME`，只记下来收尾时删会话文件用），状态目录 `state`（记号放这里）。`wiring` 是带工具的（总台里已经登记好）：套接字地址和口令放进它的环境变量，
    /// 路由和「乱说话了」的标记用总台那一份；不带的两个变量一个都不给。起不来的不删 `work`，由调用的人删（`wiring` 随之丢掉）。
    ///
    /// # Errors
    ///
    /// 起不来：原样交。
    pub fn start(
        config: &Config,
        model: &str,
        agent: &str,
        work: PathBuf,
        home: PathBuf,
        state: PathBuf,
        wiring: Option<Wiring>,
    ) -> std::io::Result<Live> {
        let mut command = agy::command(config, agy::chat_args(model, agent, &work));
        command
            .current_dir(&work)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        match &wiring {
            Some(wiring) => {
                command
                    .env(bridge::ENV_BRIDGE, wiring.socket())
                    .env(bridge::ENV_TOKEN, wiring.token());
            }
            None => {
                command
                    .env_remove(bridge::ENV_BRIDGE)
                    .env_remove(bridge::ENV_TOKEN);
            }
        }
        let mut child = process::spawn(command)?;
        let pid = child.id();
        let (route, stray, reported) = match &wiring {
            Some(wiring) => (wiring.route(), wiring.stray(), wiring.reported()),
            None => (Route::default(), Arc::default(), Arc::default()),
        };
        let agy_home = AgyHome::new(home.clone(), state, pid, &reported);
        let closed = Arc::new(AtomicBool::new(false));
        let (input, lines) = mpsc::channel();
        let writer = child
            .stdin
            .take()
            .map(|stdin| write_input(stdin, lines, pid));
        match child.stdout.take() {
            Some(stdout) => read_output(
                stdout,
                Arc::clone(&route),
                Arc::clone(&closed),
                Arc::clone(&stray),
                pid,
            ),
            None => closed.store(true, Ordering::SeqCst),
        }
        Ok(Live {
            child,
            input: Some(input),
            writer,
            route,
            closed,
            stray,
            pending: None,
            wiring,
            agent: agent.to_string(),
            conversation: None,
            work,
            home,
            agy_home,
        })
    }

    /// 进程号。
    #[must_use]
    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// agy 这段对话的编号；还不知道的是 `None`。
    #[must_use]
    pub fn conversation(&self) -> Option<&str> {
        self.conversation.as_deref()
    }

    /// 扩展自己的家目录（`HOME`，代理文件、`mcp_config.json` 在这里）。
    #[must_use]
    pub fn home(&self) -> &std::path::Path {
        &self.home
    }

    /// agy 实际用的家目录（它的转录、会话文件在这里）：它的 MCP 服务报过的用报的（包装脚本给 agy 换了 `HOME` 的情况），没报过的
    /// （请求不带工具、没起工具服务）是扩展自己的 [`Live::home`]。用的时候现取：工具服务可能在一轮里才报到。
    #[must_use]
    pub fn agy_home(&self) -> PathBuf {
        self.agy_home.current()
    }

    /// 记下 agy 这段对话的编号（收尾时删它的文件），并记号。
    pub fn set_conversation(&mut self, conversation: &str) {
        self.conversation = Some(conversation.to_string());
        self.note_conversation(conversation);
    }

    /// 刚得知对话编号 `conversation`（`init` 才到、这一轮还没完）：在状态目录里记号，扩展这时被硬杀了也有人知道该清哪段
    /// （[`AgyHome::note_conversation`]）。编号不合 [`workspace::valid_conversation`] 的不记；已经记过的不重复写；写不了的记一行日志，
    /// 不影响调用。
    pub fn note_conversation(&self, conversation: &str) {
        self.agy_home.note_conversation(conversation);
    }

    /// 之后读到的每一行送给 `to`（这一轮在听的那一次调用）。
    pub fn route(&self, to: Sender<Signal>) {
        *self.route.lock().unwrap_or_else(PoisonError::into_inner) = Some(to);
    }

    /// 这一轮完了，没人听了。
    pub fn unroute(&self) {
        *self.route.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }

    /// 把一行排进标准输入（带换行）。写的线程已经不在了（agy 先前没读就退了）的交 `false`。
    #[must_use]
    pub fn send(&self, line: String) -> bool {
        self.input
            .as_ref()
            .is_some_and(|input| input.send(line).is_ok())
    }

    /// agy 的输出读到头了（进程退了），或者读不下去了：这个进程不能再用。
    #[must_use]
    pub fn closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// 没人听的时候流里来过一行，或者总台在没人听的时候收到过调用：agy 那边已经不等我们了。
    #[must_use]
    pub fn stray(&self) -> bool {
        self.stray.load(Ordering::SeqCst)
    }

    /// 记下「乱说话了」。
    pub fn mark_stray(&self) {
        self.stray.store(true, Ordering::SeqCst);
    }

    /// 这一轮以交回一件工具调用结束：记下挂着的那件（连接跟着 `Live`）。
    pub fn set_pending(&mut self, pending: Pending) {
        self.pending = Some(pending);
    }

    /// 挂着的那件工具调用的编号；没有挂着的是 `None`。
    #[must_use]
    pub fn pending_number(&self) -> Option<u64> {
        self.pending.as_ref().map(|pending| pending.n)
    }

    /// 取走挂着的那件（要回话了，或者要丢掉）。
    pub fn take_pending(&mut self) -> Option<Pending> {
        self.pending.take()
    }
}

impl Drop for Live {
    /// 收掉：进程组（[`process::terminate`]）、写的线程、工作目录、agy 那边这段对话的文件。删不掉的记一行日志。
    fn drop(&mut self) {
        let pid = self.child.id();
        // 先让总台不再认这个口令、关掉挂着的连接，再收进程。
        drop(self.wiring.take());
        drop(self.pending.take());
        process::terminate(&mut self.child, process::GRACE);
        drop(self.input.take());
        if let Some(writer) = self.writer.take()
            && writer.join().is_err()
        {
            eprintln!("agy (pid {pid}): the thread writing its input panicked");
        }
        if let Err(error) = workspace::remove_work_dir(&self.work) {
            eprintln!(
                "agy (pid {pid}): could not remove {}: {error}",
                self.work.display()
            );
        }
        let removed = match &self.conversation {
            Some(conversation) => workspace::remove_conversation(
                &self.agy_home.current(),
                conversation,
            )
            .inspect_err(|error| {
                eprintln!(
                    "agy (pid {pid}): could not remove its conversation {conversation}: {error}"
                );
            })
            .is_ok(),
            None => true,
        };
        // 会话文件没删干净的，记号留着：下次启动的清扫再试一次。
        self.agy_home.finish(removed);
    }
}

/// 在单独的线程里把 `lines` 按先后写进 agy 的标准输入；`lines` 关了就关上标准输入。写不进去（agy 没读就退了）记一行日志、
/// 不再写。
fn write_input(mut stdin: ChildStdin, lines: Receiver<String>, pid: u32) -> JoinHandle<()> {
    std::thread::spawn(move || {
        for line in lines {
            if let Err(error) = stdin
                .write_all(line.as_bytes())
                .and_then(|()| stdin.flush())
            {
                eprintln!("agy (pid {pid}) did not take its input: {error}");
                return;
            }
        }
    })
}

/// 在单独的线程里按行读 agy 的输出，送给 `route` 现在指着的那一次调用。读到头、读不下去、一行太长：先记下 `closed`，再送
/// 最后一条，线程结束。没人听的时候来了一行：记一行日志、记下 `stray`。
fn read_output(
    stdout: ChildStdout,
    route: Route,
    closed: Arc<AtomicBool>,
    stray: Arc<AtomicBool>,
    pid: u32,
) {
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            let signal = match process::next_line(&mut reader, process::LINE_LIMIT) {
                Ok(Some(line)) => Signal::Line(line),
                Ok(None) => Signal::Closed,
                Err(error) => Signal::ReadFailed(error.to_string()),
            };
            let last = !matches!(signal, Signal::Line(Line::Text(_)));
            if last {
                closed.store(true, Ordering::SeqCst);
            }
            let route = route.lock().unwrap_or_else(PoisonError::into_inner);
            match (route.as_ref(), &signal) {
                (Some(to), _) => {
                    // 那一次已经结束、没人收了：照样往下读，下一轮会换人听。
                    to.send(signal).ok();
                }
                (None, Signal::Line(Line::Text(text))) => {
                    stray.store(true, Ordering::SeqCst);
                    eprintln!("agy (pid {pid}) wrote while idle: {text:.200}");
                }
                (None, _) => {}
            }
            if last {
                return;
            }
        }
    });
}

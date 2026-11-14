//! 一个活着的 agy 进程，也就是一段 agy 对话（设计第三节「agy 进程」、第二节「进程复用」）：连着收好几轮，几次 `driver.call`
//! 之间留着。
//!
//! - 标准输入：一个写的线程，按先后写进来的每一行；agy 没读就退出的，那一行写不进去、记一行日志，线程结束，不卡调用的人。
//! - 标准输出：一个读的线程，按行读，送给「这一轮在听的那一次调用」（[`Live::route`]）；闲着时没人听，读到了记一行日志。
//!   读到头（agy 退了）记下来（[`Live::closed`]），巡检和下一轮照它发现进程没了。
//! - 收掉（丢掉这个值时）：收进程组，删工作目录和 agy 那边这段对话的文件（设计第八节「清理」）。

use std::io::{BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;

use crate::agy;
use crate::call::Signal;
use crate::config::Config;
use crate::process::{self, Line};
use crate::workspace;

/// 读的线程把这一行送给谁。
type Route = Arc<Mutex<Option<Sender<Signal>>>>;

/// 一个活着的 agy 进程。
pub struct Live {
    child: Child,
    input: Option<Sender<String>>,
    writer: Option<JoinHandle<()>>,
    route: Route,
    closed: Arc<AtomicBool>,
    /// 这段对话用的代理名。
    agent: String,
    /// agy 这段对话的编号（`init` 里的；还不知道的是 `None`）。
    conversation: Option<String>,
    /// 给它新建的工作目录。
    work: PathBuf,
    /// 它跑在哪个家目录下（会话文件在这里）。
    home: PathBuf,
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
    /// `HOME`，只记下来收尾时删会话文件用）。起不来的不删 `work`，由调用的人删。
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
    ) -> std::io::Result<Live> {
        let mut command = agy::command(config, agy::chat_args(model, agent, &work));
        command
            .current_dir(&work)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        let mut child = process::spawn(command)?;
        let pid = child.id();
        let route: Route = Arc::new(Mutex::new(None));
        let closed = Arc::new(AtomicBool::new(false));
        let (input, lines) = mpsc::channel();
        let writer = child
            .stdin
            .take()
            .map(|stdin| write_input(stdin, lines, pid));
        match child.stdout.take() {
            Some(stdout) => read_output(stdout, Arc::clone(&route), Arc::clone(&closed), pid),
            None => closed.store(true, Ordering::SeqCst),
        }
        Ok(Live {
            child,
            input: Some(input),
            writer,
            route,
            closed,
            agent: agent.to_string(),
            conversation: None,
            work,
            home,
        })
    }

    /// 进程号。
    #[must_use]
    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// 记下 agy 这段对话的编号（收尾时删它的文件）。
    pub fn set_conversation(&mut self, conversation: &str) {
        self.conversation = Some(conversation.to_string());
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
}

impl Drop for Live {
    /// 收掉：进程组（[`process::terminate`]）、写的线程、工作目录、agy 那边这段对话的文件。删不掉的记一行日志。
    fn drop(&mut self) {
        let pid = self.child.id();
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
        if let Some(conversation) = &self.conversation
            && let Err(error) = workspace::remove_conversation(&self.home, conversation)
        {
            eprintln!("agy (pid {pid}): could not remove its conversation {conversation}: {error}");
        }
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
/// 最后一条，线程结束。
fn read_output(stdout: ChildStdout, route: Route, closed: Arc<AtomicBool>, pid: u32) {
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
                    eprintln!("agy (pid {pid}) wrote while idle: {text}");
                }
                (None, _) => {}
            }
            if last {
                return;
            }
        }
    });
}

//! 一轮的读写：先做「开头」（把一行写进活着的 agy 的标准输入；或者把工具结果经挂着的那条连接回给 agy），按行读它的流
//! （[`stream`]），照 [`Turn`] 交增量，读到 `result` 或者交回一件工具调用为止（设计第四节第 4、5 条）。复用的进程这一轮不发
//! `init`，不等它。
//!
//! agy 自己跑的工具（`search_web`）：`DONE` 了、流里没带结果的，这里先读 agy 的转录（[`crate::brain`]）把结果填进这一步再喂给
//! [`Turn`]（状态机是纯的，不碰 I/O）。读转录最多等 1 秒，这期间这个循环停着，不拿着任何锁；`driver.cancel` 最多因此晚 1 秒生效。
//!
//! 工具调用有两边：流里的工具步 `ACTIVE` 和 MCP 服务转来的 `tools/call`（[`Signal::ToolCall`]），谁先到都可能。第一边到了就开始
//! 掐表（[`PAIR_WAIT`]），到点还没等到另一边就照 [`Turn::timeout`]。等的时候流里来了别的事照常处理。

use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::{Signal, cancelled};
use crate::brain;
use crate::bridge::Reply;
use crate::hub::McpCall;
use crate::live::{Live, Pending};
use crate::process::{self, Line};
use crate::rpc::Out;
use crate::stream;
use crate::stream::{ToolState, ToolStep};
use crate::turn::{Fed, MCP_PREFIX, Turn};

/// 工具调用只到了一边时等另一边多久（设计第四节第 4 条）。
pub const PAIR_WAIT: Duration = Duration::from_secs(10);

/// 改 [`PAIR_WAIT`] 的环境变量（毫秒）：只给测试用，不是配置项。
pub const PAIR_WAIT_ENV: &str = "MIYU_ANTIGRAVITY_PAIR_WAIT_MS";

/// 等另一边多久：环境变量 [`PAIR_WAIT_ENV`] 给了正整数的照它，别的照 [`PAIR_WAIT`]。
#[must_use]
pub fn pair_wait() -> Duration {
    std::env::var(PAIR_WAIT_ENV)
        .ok()
        .and_then(|text| text.parse::<u64>().ok())
        .filter(|millis| *millis > 0)
        .map_or(PAIR_WAIT, Duration::from_millis)
}

/// 一轮怎么结束的。
#[derive(Debug)]
pub enum Ended {
    /// 有了回答：`result`（成功的、出错的）、被取消、流里出了错。
    Answer(Value),
    /// agy 一行都没写就没了（退了、写不进标准输入）：回答是照没交增量归类的出错。接在复用的进程上的，可以改成新开。
    Silent(Value),
    /// 交回了一件工具调用：回答（用量）和挂着的那件调用（agy 还等着结果，连接在里面）。
    Handed(Value, Pending),
}

impl Ended {
    /// 回答本身（挂着的调用随之丢掉，连接关掉）。
    pub fn into_answer(self) -> Value {
        match self {
            Ended::Answer(answer) | Ended::Silent(answer) | Ended::Handed(answer, _) => answer,
        }
    }
}

/// 回答成功了没有（没有 `error`）。
#[must_use]
pub fn succeeded(answer: &Value) -> bool {
    answer.get("error").is_none()
}

/// 这一次调用（编号 `call`）在 `live` 上说一轮：读到的行经 `sender` 送进 `signals`，增量经 `out` 交。`start` 是这一轮的开头
/// （写标准输入、回工具结果），**在接上路由以后**才做，免得 agy 马上说的话没人收；它交 `Err(原因)` 的，这一轮是 [`Ended::Silent`]。
pub fn talk(
    out: &Out,
    call: &str,
    live: &Live,
    turn: &mut Turn,
    signals: &Receiver<Signal>,
    sender: &Sender<Signal>,
    start: impl FnOnce() -> Result<(), String>,
) -> Ended {
    live.route(sender.clone());
    // agy 在「看过它还活着」和「接上路由」之间退了的：读输出的线程已经记下 `closed`、把最后一条送给了没人听的路由，这一轮
    // 再也等不来信号。接上以后再看一眼，自己补这一条（读线程是先记 `closed` 再送的，所以要么这里看得到，要么它送得到）。
    if live.closed() {
        sender.send(Signal::Closed).ok();
    }
    let source = Transcripts {
        live,
        conversation: live.conversation().map(str::to_string),
    };
    let ended = match start() {
        Ok(()) => listen(out, call, turn, signals, &source),
        Err(why) => Ended::Silent(broken(out, call, turn, &why)),
    };
    live.unroute();
    if matches!(ended, Ended::Handed(..)) {
        // 交回以后 agy 不该再说话：路由撤掉之前就送进来、还没收的，说明它已经不等了。
        while let Ok(signal) = signals.try_recv() {
            if !matches!(signal, Signal::Cancel) {
                live.mark_stray();
            }
        }
    }
    ended
}

/// 写一行进标准输入当这一轮的开头。
pub fn write_input(live: &Live, input: String) -> impl FnOnce() -> Result<(), String> + '_ {
    move || {
        if live.send(input) {
            Ok(())
        } else {
            Err("agy stopped taking input".to_string())
        }
    }
}

/// 去哪读 agy 的转录：它实际用的家目录（[`Live::agy_home`]，真去读的那一刻才取：工具服务可能在这一轮里才报到），和（复用的
/// 进程这一轮不发 `init`）`Live` 记着的对话编号。
struct Transcripts<'a> {
    /// 这一轮说话的 agy：得知它的对话编号时记号。
    live: &'a Live,
    conversation: Option<String>,
}

impl Transcripts<'_> {
    /// 一步 agy 自己跑完了的工具（`DONE`、不是 Miyu 的 MCP 工具）、流里没带结果的：从转录读结果填进去。读不到的照样，由状态机
    /// 只收块、不交结果。对话编号先看这一轮的 `init`，没有的用 `Live` 里的。
    fn fill(&self, call: &str, turn: &Turn, step: &mut ToolStep) {
        let missing = step.output.as_deref().is_none_or(str::is_empty);
        if step.state != ToolState::Done || step.name.starts_with(MCP_PREFIX) || !missing {
            return;
        }
        let Some(conversation) = turn.conversation().or(self.conversation.as_deref()) else {
            eprintln!(
                "call {call}: agy ran {} but its conversation id is unknown; no result to hand back",
                step.name
            );
            return;
        };
        step.output = brain::read_waiting(
            &self.live.agy_home(),
            conversation,
            step.step_index,
            brain::WAIT,
            brain::POLL,
        );
        if step.output.is_none() {
            eprintln!(
                "call {call}: agy's transcript has no result for step {} ({}); handing back the block without one",
                step.step_index, step.name
            );
        }
    }
}

/// 流断了、没有 `result`：攒着没交的字先交掉，再给照 [`Turn::broken`] 归的回答。
fn broken(out: &Out, call: &str, turn: &mut Turn, why: &str) -> Value {
    deliver(out, call, turn.flush());
    turn.broken(why)
}

/// 读到这一轮有回答、或者交回了一件工具调用为止。
fn listen(
    out: &Out,
    call: &str,
    turn: &mut Turn,
    signals: &Receiver<Signal>,
    source: &Transcripts,
) -> Ended {
    let mut heard = false;
    let mut held: Option<(u64, Reply)> = None;
    let mut deadline: Option<Instant> = None;
    loop {
        let signal = match deadline {
            None => signals.recv().unwrap_or(Signal::Closed),
            Some(at) => match signals.recv_timeout(at.saturating_duration_since(Instant::now())) {
                Ok(signal) => signal,
                Err(RecvTimeoutError::Timeout) => {
                    if let Some(ended) = settle(out, call, turn.timeout(), &mut held) {
                        return ended;
                    }
                    deadline = None;
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => Signal::Closed,
            },
        };
        let why = match signal {
            Signal::Cancel => {
                deliver(out, call, turn.flush());
                return Ended::Answer(cancelled());
            }
            Signal::ToolCall(McpCall {
                n,
                name,
                arguments,
                reply,
            }) => {
                heard = true;
                eprintln!("call {call}: agy called tool {name} (#{n}) through the tool server");
                if held.is_some() {
                    eprintln!(
                        "call {call}: a second tool call arrived while one was waiting; refusing it"
                    );
                    continue;
                }
                held = Some((n, reply));
                let fed = turn.tool_call(n, &name, &arguments);
                if let Some(ended) = settle(out, call, fed, &mut held) {
                    return ended;
                }
                arm(turn, &mut deadline);
                continue;
            }
            Signal::Line(Line::Text(text)) => {
                heard = true;
                match stream::parse(&text) {
                    Ok(mut event) => {
                        if let stream::Event::Tool(step) = &mut event {
                            eprintln!("call {call}: agy {}", step.describe());
                            source.fill(call, turn, step);
                        }
                        let fed = turn.feed(event);
                        if let Some(conversation) = turn.conversation() {
                            source.live.note_conversation(conversation);
                        }
                        if let Some(ended) = settle(out, call, fed, &mut held) {
                            return ended;
                        }
                        arm(turn, &mut deadline);
                    }
                    Err(error) => eprintln!("call {call}: {error}"),
                }
                continue;
            }
            Signal::Line(Line::TooLong) => {
                return Ended::Answer(broken(
                    out,
                    call,
                    turn,
                    &format!("agy wrote a line longer than {} bytes", process::LINE_LIMIT),
                ));
            }
            Signal::ReadFailed(error) => format!("reading agy's output failed: {error}"),
            Signal::Closed => "agy exited without a result".to_string(),
        };
        let answer = broken(out, call, turn, &why);
        return if heard {
            Ended::Answer(answer)
        } else {
            Ended::Silent(answer)
        };
    }
}

/// 第一边到了就开始掐表；已经在掐的不重新算。
fn arm(turn: &Turn, deadline: &mut Option<Instant>) {
    if deadline.is_none() && turn.waiting().is_some() {
        *deadline = Some(Instant::now() + pair_wait());
    }
}

/// 交出 `fed` 的增量；有了回答的，这一轮就结束了：交回工具调用的带上手里的连接，别的就是回答本身。
fn settle(out: &Out, call: &str, fed: Fed, held: &mut Option<(u64, Reply)>) -> Option<Ended> {
    let Fed {
        deltas,
        answer,
        handed,
    } = fed;
    deliver(out, call, deltas);
    let answer = answer?;
    let Some(n) = handed else {
        return Some(Ended::Answer(answer));
    };
    match held.take() {
        Some((held_n, reply)) if held_n == n => Some(Ended::Handed(answer, Pending { n, reply })),
        _ => Some(Ended::Answer(crate::turn::failed(
            crate::failure::Class::Unclassified,
            "internal error: a tool call was handed back without its connection",
        ))),
    }
}

/// 把增量交给核心：一条 `driver.delta` 至多带一段字，一行不会太长。
fn deliver(out: &Out, call: &str, deltas: Vec<Value>) {
    let mut batch: Vec<Value> = Vec::new();
    for delta in deltas {
        if delta.get("text").is_some() && batch.iter().any(|queued| queued.get("text").is_some()) {
            out.notify(
                "driver.delta",
                &json!({"call": call, "deltas": std::mem::take(&mut batch)}),
            );
        }
        batch.push(delta);
    }
    if !batch.is_empty() {
        out.notify("driver.delta", &json!({"call": call, "deltas": batch}));
    }
}

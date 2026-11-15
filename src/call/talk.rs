//! 一轮的读写：先做「开头」（把一行写进活着的 agy 的标准输入；或者把工具结果经挂着的那条连接回给 agy），按行读它的流
//! （[`stream`]），照 [`Turn`] 交增量，读到 `result` 或者交回一件工具调用为止（设计第四节第 4、5 条）。复用的进程这一轮不发
//! `init`，不等它。
//!
//! 工具调用有两边：流里的工具步 `ACTIVE` 和 MCP 服务转来的 `tools/call`（[`Signal::ToolCall`]），谁先到都可能。第一边到了就开始
//! 掐表（[`PAIR_WAIT`]），到点还没等到另一边就照 [`Turn::timeout`]。等的时候流里来了别的事照常处理。

use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::{Signal, cancelled};
use crate::bridge::Reply;
use crate::hub::McpCall;
use crate::live::{Live, Pending};
use crate::process::{self, Line};
use crate::rpc::Out;
use crate::stream;
use crate::turn::{Fed, Turn};

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
    let ended = match start() {
        Ok(()) => listen(out, call, turn, signals),
        Err(why) => Ended::Silent(turn.broken(&why)),
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

/// 读到这一轮有回答、或者交回了一件工具调用为止。
fn listen(out: &Out, call: &str, turn: &mut Turn, signals: &Receiver<Signal>) -> Ended {
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
            Signal::Cancel => return Ended::Answer(cancelled()),
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
                    Ok(event) => {
                        if let stream::Event::Tool(step) = &event {
                            eprintln!("call {call}: agy {}", step.describe());
                        }
                        let fed = turn.feed(event);
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
                return Ended::Answer(turn.broken(&format!(
                    "agy wrote a line longer than {} bytes",
                    process::LINE_LIMIT
                )));
            }
            Signal::ReadFailed(error) => format!("reading agy's output failed: {error}"),
            Signal::Closed => "agy exited without a result".to_string(),
        };
        let answer = turn.broken(&why);
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

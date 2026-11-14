//! 一轮的读写：把一行写进活着的 agy 的标准输入，按行读它的流（[`stream`]），照 [`Turn`] 交增量，读到 `result` 为止
//! （设计第四节第 4、5 条）。复用的进程这一轮不发 `init`，不等它。

use std::sync::mpsc::{Receiver, Sender};

use serde_json::{Value, json};

use super::{Signal, cancelled};
use crate::live::Live;
use crate::process::{self, Line};
use crate::rpc::Out;
use crate::stream;
use crate::turn::Turn;

/// 一轮怎么结束的。
#[derive(Debug, PartialEq)]
pub enum Ended {
    /// 有了回答：`result`（成功的、出错的）、被取消、流里出了错。
    Answer(Value),
    /// agy 一行都没写就没了（退了、写不进标准输入）：回答是照没交增量归类的出错。接在复用的进程上的，可以改成新开。
    Silent(Value),
}

impl Ended {
    /// 回答本身。
    pub fn into_answer(self) -> Value {
        match self {
            Ended::Answer(answer) | Ended::Silent(answer) => answer,
        }
    }
}

/// 回答成功了没有（没有 `error`）。
#[must_use]
pub fn succeeded(answer: &Value) -> bool {
    answer.get("error").is_none()
}

/// 这一次调用（编号 `call`）在 `live` 上说一轮：读到的行经 `sender` 送进 `signals`，`input` 写进标准输入，增量经 `out` 交。
pub fn talk(
    out: &Out,
    call: &str,
    live: &Live,
    turn: &mut Turn,
    input: String,
    signals: &Receiver<Signal>,
    sender: &Sender<Signal>,
) -> Ended {
    live.route(sender.clone());
    let ended = if live.send(input) {
        listen(out, call, turn, signals)
    } else {
        Ended::Silent(turn.broken("agy stopped taking input"))
    };
    live.unroute();
    ended
}

/// 读到这一轮有回答为止。
fn listen(out: &Out, call: &str, turn: &mut Turn, signals: &Receiver<Signal>) -> Ended {
    let mut heard = false;
    loop {
        let signal = signals.recv().unwrap_or(Signal::Closed);
        let why = match signal {
            Signal::Cancel => return Ended::Answer(cancelled()),
            Signal::Line(Line::Text(text)) => {
                heard = true;
                match stream::parse(&text) {
                    Ok(event) => {
                        if let stream::Event::Tool(step) = &event {
                            // AG-2 还不管工具步（代理文件把原生工具全关了）；跑了的记一行，看得出 agy 做了什么。
                            eprintln!("call {call}: agy {}", step.describe());
                        }
                        let fed = turn.feed(event);
                        deliver(out, call, fed.deltas);
                        if let Some(answer) = fed.answer {
                            return Ended::Answer(answer);
                        }
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

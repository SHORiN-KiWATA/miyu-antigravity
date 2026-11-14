//! 一次 `driver.call`（AG-1：没有工具、每次新开一段 agy 对话，设计第四节第 3、4 条，第五节，第八节）：
//!
//! 1. 整份 `request.messages` 转写成一条文字（[`transcript`]），`request.system` 写成代理（[`agent`]）；
//! 2. 在状态目录下新建一个空的工作目录，照 [`agy::chat_args`] 起 agy（[`process`]），提示词在单独的线程里写进标准输入；
//! 3. 按行读流（[`stream`]），照 [`Turn`] 交增量，读到 `result` 就回答；
//! 4. 不管成败：收掉 agy 的进程组，删掉这段 agy 对话的文件和工作目录。
//!
//! `driver.cancel` 经 [`Signal::Cancel`] 送进同一个通道：收掉进程，这一次回一个出错（核心已经不理了）。

use std::io::{BufReader, Write};
use std::path::Path;
use std::process::{Child, Stdio};
use std::sync::mpsc::{Receiver, Sender};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::config::{self, Config};
use crate::failure::Class;
use crate::process::{self, Line};
use crate::rpc::Out;
use crate::turn::{self, Turn};
use crate::{agent, agy, stream, transcript, workspace};

/// 送给一次调用的事：agy 的输出、读的结局、核心的取消。
#[derive(Debug)]
pub enum Signal {
    /// agy 的一行（或者太长了）。
    Line(Line),
    /// 读 agy 的输出出错了。
    ReadFailed(String),
    /// agy 的输出读到头了。
    Closed,
    /// 核心发了 `driver.cancel`，或者扩展要退出了。
    Cancel,
}

/// 这一次怎么回：照常的 `result`，或者 JSON-RPC 的出错（参数不对）。
#[derive(Debug, PartialEq)]
pub enum Answer {
    /// `result`：`{"usage"?, "error"?, "sent"?}`。
    Result(Value),
    /// JSON-RPC 出错：参数读不成（`bad_params`）。
    BadParams(String),
}

/// 起不来 agy 时给人看的一句。
#[must_use]
pub fn spawn_failure(config: &Config, error: &std::io::Error) -> String {
    let program = config.program();
    if error.kind() == std::io::ErrorKind::NotFound {
        let place = if config.binary.is_some() {
            format!("at {}", program.display())
        } else {
            "on PATH".to_string()
        };
        format!(
            "agy (the Antigravity CLI) was not found {place}; install it or set {} to its full path",
            config::BINARY
        )
    } else {
        format!("could not start agy ({}): {error}", program.display())
    }
}

/// 跑一次 `driver.call`：参数 `params`，增量经 `out` 交，`signals` 收 agy 的输出和取消（`sender` 是它的发送头，交给读输出的
/// 线程）。
pub fn run(
    out: &Out,
    config: &Config,
    params: &Value,
    signals: &Receiver<Signal>,
    sender: &Sender<Signal>,
) -> Answer {
    let Some(call) = params.get("call").and_then(Value::as_str) else {
        return Answer::BadParams("params.call is missing".into());
    };
    let Some(model) = params.pointer("/params/model").and_then(Value::as_str) else {
        return Answer::BadParams("params.params.model is missing".into());
    };
    let Some(request) = params.get("request") else {
        return Answer::BadParams("params.request is missing".into());
    };
    let Some(system) = request.get("system").and_then(Value::as_str) else {
        return Answer::BadParams("params.request.system is not text".into());
    };
    let text = match transcript::transcribe(request, transcript::BUDGET) {
        Ok(text) => text,
        Err(error) => return Answer::BadParams(error.to_string()),
    };
    let line = agy::stdin_line(&text);
    let sent = format!("sha256:{}", agent::hex(&Sha256::digest(line.as_bytes())));

    let Some(home) = workspace::home() else {
        return Answer::Result(turn::failed(
            Class::Unclassified,
            "HOME is not set, so there is no home folder to run agy in",
        ));
    };
    let agent = match agent::write(&home, system) {
        Ok(agent) => agent,
        Err(error) => {
            return Answer::Result(turn::failed(
                Class::Unclassified,
                &format!(
                    "could not write the agy agent file under {}: {error}",
                    home.display()
                ),
            ));
        }
    };
    let work = match workspace::state_dir().and_then(|state| workspace::create_work_dir(&state)) {
        Ok(work) => work,
        Err(error) => {
            return Answer::Result(turn::failed(
                Class::Unclassified,
                &format!("could not make a working folder for agy: {error}"),
            ));
        }
    };

    let mut turn = Turn::new(&agent);
    let job = Job {
        out,
        config,
        call,
        model,
        work: &work,
        line: &line,
    };
    let mut answer = converse(&job, &mut turn, signals, sender);
    if let Some(object) = answer.as_object_mut()
        && !object.contains_key("error")
    {
        object.insert("sent".into(), Value::String(sent));
    }

    if let Err(error) = workspace::remove_work_dir(&work) {
        eprintln!("call {call}: could not remove {}: {error}", work.display());
    }
    if let Some(conversation) = turn.conversation()
        && let Err(error) = workspace::remove_conversation(&home, conversation)
    {
        eprintln!("call {call}: could not remove agy conversation {conversation}: {error}");
    }
    Answer::Result(answer)
}

/// 这一次要用的：往哪交、配置、调用编号、模型、工作目录、写进标准输入的那一行。
struct Job<'a> {
    out: &'a Out,
    config: &'a Config,
    call: &'a str,
    model: &'a str,
    work: &'a Path,
    line: &'a str,
}

/// 起 agy、写提示词、读流，交回答；返回之前收掉 agy。
fn converse(
    job: &Job<'_>,
    turn: &mut Turn,
    signals: &Receiver<Signal>,
    sender: &Sender<Signal>,
) -> Value {
    let Job {
        out,
        config,
        call,
        model,
        work,
        line,
    } = *job;
    if let Ok(Signal::Cancel) = signals.try_recv() {
        return cancelled();
    }
    let mut command = agy::command(config, agy::chat_args(model, turn.agent(), work));
    command
        .current_dir(work)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    let mut child = match process::spawn(command) {
        Ok(child) => child,
        Err(error) => return turn::failed(Class::Unclassified, &spawn_failure(config, &error)),
    };
    let writer = write_prompt(&mut child, line.to_string(), call.to_string());
    read_output(&mut child, sender.clone());

    let answer = loop {
        let signal = signals.recv().unwrap_or(Signal::Closed);
        match signal {
            Signal::Cancel => break cancelled(),
            Signal::Line(Line::Text(text)) => match stream::parse(&text) {
                Ok(event) => {
                    if let stream::Event::Tool(step) = &event {
                        // AG-1 不管工具步（代理文件把原生工具全关了）；跑了的记一行，看得出 agy 做了什么。
                        eprintln!("call {call}: agy {}", step.describe());
                    }
                    let fed = turn.feed(event);
                    deliver(out, call, fed.deltas);
                    if let Some(answer) = fed.answer {
                        break answer;
                    }
                }
                Err(error) => eprintln!("call {call}: {error}"),
            },
            Signal::Line(Line::TooLong) => {
                break turn.broken(&format!(
                    "agy wrote a line longer than {} bytes",
                    process::LINE_LIMIT
                ));
            }
            Signal::ReadFailed(error) => {
                break turn.broken(&format!("reading agy's output failed: {error}"));
            }
            Signal::Closed => break turn.broken("agy exited without a result"),
        }
    };
    process::terminate(&mut child, process::GRACE);
    if writer.join().is_err() {
        eprintln!("call {call}: the thread writing agy's input panicked");
    }
    answer
}

/// 被取消的这一次的回答：核心已经不理了，照样回一个，请求有始有终。
fn cancelled() -> Value {
    turn::failed(Class::Retryable, "the call was cancelled")
}

/// 在单独的线程里把提示词写进 agy 的标准输入，写完关上（agy 可能没读就退出了，写不进不算错，记一行日志）。
fn write_prompt(child: &mut Child, line: String, call: String) -> std::thread::JoinHandle<()> {
    let stdin = child.stdin.take();
    std::thread::spawn(move || {
        let Some(mut stdin) = stdin else {
            return;
        };
        if let Err(error) = stdin
            .write_all(line.as_bytes())
            .and_then(|()| stdin.flush())
        {
            eprintln!("call {call}: agy did not take its input: {error}");
        }
    })
}

/// 在单独的线程里按行读 agy 的输出，送进 `sender`。
fn read_output(child: &mut Child, sender: Sender<Signal>) {
    let Some(stdout) = child.stdout.take() else {
        sender.send(Signal::Closed).ok();
        return;
    };
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            let signal = match process::next_line(&mut reader, process::LINE_LIMIT) {
                Ok(Some(Line::TooLong)) => Signal::Line(Line::TooLong),
                Ok(Some(line)) => Signal::Line(line),
                Ok(None) => Signal::Closed,
                Err(error) => Signal::ReadFailed(error.to_string()),
            };
            let last = !matches!(signal, Signal::Line(Line::Text(_)));
            // 这一次已经结束、没人收了：不用再读。
            if sender.send(signal).is_err() || last {
                return;
            }
        }
    });
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

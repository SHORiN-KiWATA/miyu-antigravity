//! 一次 `driver.call`（设计第四节第 1～5 条，第五节，第八节）：
//!
//! 1. 认是哪段对话（[`crate::sessions::next_turn`]）：登记表（[`Registry`]）里有闲着的、接得上的，就是**新的一轮**——只把
//!    新的几条 user 消息写进那个还活着的 agy 的标准输入（[`transcript::new_turn`]），读流、交增量（[`talk`]）；成功了记下
//!    这一次的消息指纹、放回表里。那个 agy 一行没写就没了的，改走新开。
//! 2. 认不上的**新开**：整份 `request.messages` 转写成一条文字（[`transcript::transcribe`]），`request.system` 写成代理
//!    （[`agent`]），新建工作目录，起 agy（[`Live`]），读流、交增量；成功了登记进表（闲着），agy 留着等下一轮。
//! 3. 出了任何错（含 `driver.cancel`）：这段 agy 对话作废——收进程组、删会话文件和工作目录（丢掉 [`Live`] 就是收掉）。
//!
//! `driver.cancel` 经 [`Signal::Cancel`] 送进同一个通道。

mod talk;

use std::sync::mpsc::{Receiver, Sender};
use std::time::{Instant, SystemTime};

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::config::{self, Config};
use crate::failure::Class;
use crate::live::Live;
use crate::process::Line;
use crate::registry::Registry;
use crate::rpc::Out;
use crate::sessions::{Claimed, Identity, Incoming, Meta};
use crate::turn::{self, Turn};
use crate::{agent, agy, transcript, workspace};
use talk::{Ended, succeeded, talk};

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

/// 这一次要用的：往哪交、配置、登记表、调用编号、模型、系统提示词、请求、收信号的两头。
struct Job<'a> {
    out: &'a Out,
    config: &'a Config,
    registry: &'a Registry,
    call: &'a str,
    model: &'a str,
    system: &'a str,
    request: &'a Value,
    signals: &'a Receiver<Signal>,
    sender: &'a Sender<Signal>,
}

/// 跑一次 `driver.call`：参数 `params`，增量经 `out` 交，`signals` 收 agy 的输出和取消（`sender` 是它的发送头，交给读输出的
/// 线程）；活着的 agy 对话在 `registry` 里。
pub fn run(
    out: &Out,
    config: &Config,
    registry: &Registry,
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
    // 先整份转写一遍：请求的形状不对的当场拒，新开时也要用。
    let whole = match transcript::transcribe(request, transcript::BUDGET) {
        Ok(text) => text,
        Err(error) => return Answer::BadParams(error.to_string()),
    };
    let incoming = incoming(params, model, request);
    if incoming.is_none() {
        eprintln!("call {call}: no usable fingerprint; this agy conversation will not be kept");
    }
    let job = Job {
        out,
        config,
        registry,
        call,
        model,
        system,
        request,
        signals,
        sender,
    };
    if let Some(incoming) = &incoming
        && let Some(claimed) = registry.claim(incoming)
    {
        match resume(&job, claimed, incoming) {
            Ok(answer) => return Answer::Result(answer),
            Err(gone) => eprintln!("call {call}: {gone}; starting a new agy conversation"),
        }
    }
    Answer::Result(fresh(&job, &whole, incoming))
}

/// 认对话要的：模型、`fingerprint` 的三样、每条消息的 `role`。`fingerprint` 缺了、写法不对、消息指纹和消息数对不上的是
/// `None`（这一次照新开，说完不留）。
fn incoming(params: &Value, model: &str, request: &Value) -> Option<Incoming> {
    let fingerprint = params.get("fingerprint")?;
    let text = |key: &str| fingerprint.get(key)?.as_str().map(str::to_string);
    let messages: Vec<String> = fingerprint
        .get("messages")?
        .as_array()?
        .iter()
        .map(|value| value.as_str().map(str::to_string))
        .collect::<Option<_>>()?;
    let roles: Vec<String> = request
        .get("messages")?
        .as_array()?
        .iter()
        .map(|message| message.get("role")?.as_str().map(str::to_string))
        .collect::<Option<_>>()?;
    if messages.len() != roles.len() {
        return None;
    }
    Some(Incoming {
        identity: Identity {
            model: model.to_string(),
            system: text("system")?,
            tools: text("tools")?,
        },
        messages,
        roles,
    })
}

/// 成功的回答带上 `sent`：写进标准输入的那一行 `input` 的 SHA-256。
fn with_sent(mut answer: Value, input: &str) -> Value {
    if let Some(object) = answer.as_object_mut()
        && !object.contains_key("error")
    {
        let sent = format!("sha256:{}", agent::hex(&Sha256::digest(input.as_bytes())));
        object.insert("sent".into(), Value::String(sent));
    }
    answer
}

/// 新的一轮：接在 `claimed` 那个活着的 agy 上。交回答；那个 agy 一行没写就没了的交 `Err`（原因），这段已经作废，调用的人
/// 改走新开。
fn resume(job: &Job<'_>, claimed: Claimed<Live>, incoming: &Incoming) -> Result<Value, String> {
    let Claimed {
        id,
        live,
        known,
        agent,
    } = claimed;
    let texts = match transcript::new_turn(job.request, known + 1) {
        Ok(texts) => texts,
        Err(error) => {
            // 整份转写已经读通过，这里不会出错；真出了照作废。
            job.registry.forget(id);
            drop(live);
            return Ok(turn::failed(Class::Unclassified, &error.to_string()));
        }
    };
    if let Some(home) = workspace::home()
        && let Err(error) = agent::touch(&home, &agent)
    {
        eprintln!(
            "call {}: could not touch agy agent {agent}: {error}",
            job.call
        );
    }
    let input = agy::stdin_texts(&texts);
    let mut turn = Turn::new(&agent);
    let ended = talk(
        job.out,
        job.call,
        &live,
        &mut turn,
        input.clone(),
        job.signals,
        job.sender,
    );
    match ended {
        Ended::Answer(answer) if succeeded(&answer) => {
            job.registry.release(id, live, incoming.messages.clone());
            Ok(with_sent(answer, &input))
        }
        Ended::Answer(answer) => {
            job.registry.forget(id);
            drop(live);
            Ok(answer)
        }
        Ended::Silent(answer) => {
            let pid = live.pid();
            job.registry.forget(id);
            drop(live);
            Err(format!("the kept agy (pid {pid}) was gone ({answer})"))
        }
    }
}

/// 新开一段 agy 对话：整份转写 `whole` 发过去。成功了、认得出对话（`incoming`）的登记进表，agy 留着；别的都收掉。
fn fresh(job: &Job<'_>, whole: &str, incoming: Option<Incoming>) -> Value {
    let Some(home) = workspace::home() else {
        return turn::failed(
            Class::Unclassified,
            "HOME is not set, so there is no home folder to run agy in",
        );
    };
    let agent = match agent::write(&home, job.system) {
        Ok(agent) => agent,
        Err(error) => {
            return turn::failed(
                Class::Unclassified,
                &format!(
                    "could not write the agy agent file under {}: {error}",
                    home.display()
                ),
            );
        }
    };
    let in_use = job.registry.agents();
    if let Err(error) = agent::sweep(&home, &in_use, SystemTime::now(), agent::MAX_AGE) {
        eprintln!("call {}: cleaning old agy agents failed: {error}", job.call);
    }
    if let Ok(Signal::Cancel) = job.signals.try_recv() {
        return cancelled();
    }
    let work = match workspace::state_dir().and_then(|state| workspace::create_work_dir(&state)) {
        Ok(work) => work,
        Err(error) => {
            return turn::failed(
                Class::Unclassified,
                &format!("could not make a working folder for agy: {error}"),
            );
        }
    };
    let mut live = match Live::start(job.config, job.model, &agent, work.clone(), home) {
        Ok(live) => live,
        Err(error) => {
            if let Err(error) = workspace::remove_work_dir(&work) {
                eprintln!(
                    "call {}: could not remove {}: {error}",
                    job.call,
                    work.display()
                );
            }
            return turn::failed(Class::Unclassified, &spawn_failure(job.config, &error));
        }
    };
    let input = agy::stdin_line(whole);
    let mut turn = Turn::new(&agent);
    let answer = talk(
        job.out,
        job.call,
        &live,
        &mut turn,
        input.clone(),
        job.signals,
        job.sender,
    )
    .into_answer();
    if let Some(conversation) = turn.conversation() {
        live.set_conversation(conversation);
    }
    if !succeeded(&answer) {
        drop(live);
        return answer;
    }
    match incoming {
        Some(incoming) => job.registry.insert(
            Meta {
                identity: incoming.identity,
                messages: incoming.messages,
                agent,
                used: Instant::now(),
            },
            live,
        ),
        None => drop(live),
    }
    with_sent(answer, &input)
}

/// 被取消的这一次的回答：核心已经不理了，照样回一个，请求有始有终。
fn cancelled() -> Value {
    turn::failed(Class::Retryable, "the call was cancelled")
}

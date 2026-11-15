//! 一次 `driver.call`（设计第四节第 1～5 条，第五节，第八节）：
//!
//! 1. 认是哪段对话（[`crate::sessions::next_turn`]）：登记表（[`Registry`]）里有闲着的、接得上的，就是**新的一轮**——只把
//!    新的几条 user 消息写进那个还活着的 agy 的标准输入（[`transcript::new_turn`]），读流、交增量（`call/talk.rs`）；成功了记下
//!    这一次的消息指纹、放回表里。那个 agy 一行没写就没了的，改走新开。
//! 2. 认不上的**新开**：整份 `request.messages` 转写成一条文字（[`transcript::transcribe`]），`request.system` 写成代理
//!    （[`agent`]），新建工作目录，起 agy（[`Live`]），读流、交增量；成功了登记进表（闲着），agy 留着等下一轮。
//! 3. 出了任何错（含 `driver.cancel`）：这段 agy 对话作废——收进程组、删会话文件和工作目录（丢掉 [`Live`] 就是收掉）。
//!
//! 请求带工具（`request.tools`）的，agy 经 MCP 服务调 Miyu 的工具（设计第四节第 4 条、第六节）：新开时先把工具表登记到总台
//! （[`crate::hub`]）、把这个包的 MCP 条目写进 agy 的 `mcp_config.json`（[`crate::mcp_config`]），再起 agy。agy 一调工具，这一次
//! `driver.call` 就以一块工具调用结束，agy 那头挂着等（[`Live`] 带着挂着的调用，表里记成「挂着」）；下一次请求带着
//! assistant（那块工具调用）加 tool（结果）来，认得出就把结果回给 agy、接着读流（`call/resume.rs`）；对不上的挂着的作废、照第五节重放。
//!
//! `driver.cancel` 经 [`Signal::Cancel`] 送进同一个通道。

mod resume;
mod talk;

use std::path::Path;
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Instant, SystemTime};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::bridge::Tool;
use crate::config::{self, Config};
use crate::failure::Class;
use crate::hub::{McpCall, Tools, Wiring};
use crate::live::Live;
use crate::mcp_config::{self, Registration};
use crate::process::Line;
use crate::registry::Registry;
use crate::rpc::Out;
use crate::sessions::{Identity, Incoming, Meta};
use crate::turn::{self, Turn};
use crate::{agent, agy, schema, transcript, workspace};
use resume::{resume, resume_pending};
use talk::{Ended, succeeded, talk, write_input};

/// 送给一次调用的事：agy 的输出、读的结局、核心的取消。
#[derive(Debug)]
pub enum Signal {
    /// agy 的一行（或者太长了）。
    Line(Line),
    /// 读 agy 的输出出错了。
    ReadFailed(String),
    /// agy 的输出读到头了。
    Closed,
    /// agy 经 MCP 服务调了一件 Miyu 的工具（总台转来的，连接在里面）。
    ToolCall(McpCall),
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

/// 这一次要用的：往哪交、配置、登记表、工具这一头、调用编号、模型、系统提示词、请求、请求里的工具表（整过形的）、收信号的两头。
struct Job<'a> {
    out: &'a Out,
    config: &'a Config,
    registry: &'a Registry,
    tools: &'a Tools,
    call: &'a str,
    model: &'a str,
    system: &'a str,
    request: &'a Value,
    declared: Vec<Tool>,
    signals: &'a Receiver<Signal>,
    sender: &'a Sender<Signal>,
}

/// 跑一次 `driver.call`：参数 `params`，增量经 `out` 交，`signals` 收 agy 的输出、MCP 转来的工具调用和取消（`sender` 是它的
/// 发送头，交给读输出的线程）；活着的 agy 对话在 `registry` 里，工具这一头（总台和套接字）是 `tools`。
pub fn run(
    out: &Out,
    config: &Config,
    registry: &Registry,
    tools: &Tools,
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
    let declared = match declared_tools(request) {
        Ok(declared) => declared,
        Err(message) => return Answer::BadParams(message),
    };
    let incoming = incoming(params, model, request);
    if incoming.is_none() {
        eprintln!("call {call}: no usable fingerprint; this agy conversation will not be kept");
    }
    let job = Job {
        out,
        config,
        registry,
        tools,
        call,
        model,
        system,
        request,
        declared,
        signals,
        sender,
    };
    if let Some(incoming) = &incoming
        && let Some(claimed) = registry.claim(incoming)
    {
        let resumed = if claimed.pending {
            resume_pending(&job, claimed, incoming)
        } else {
            resume(&job, claimed, incoming)
        };
        match resumed {
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

/// 请求里声明的工具：名字、说明、参数格式（照 [`schema::shape`] 整过形）。写法不对的交说明哪里不对。
fn declared_tools(request: &Value) -> Result<Vec<Tool>, String> {
    let Some(declared) = request.get("tools").and_then(Value::as_array) else {
        return Ok(Vec::new());
    };
    declared
        .iter()
        .enumerate()
        .map(|(index, tool)| {
            let name = tool
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("params.request.tools[{index}].name is not text"))?;
            Ok(Tool {
                name: name.to_string(),
                description: tool
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                input_schema: schema::shape(
                    tool.get("parameters").cloned().unwrap_or_else(|| json!({})),
                ),
            })
        })
        .collect()
}

/// 成功的回答带上 `sent`：写进标准输入的那一行 `input` 的 SHA-256（回工具结果的那一轮是回给 agy 的那段字）。
fn with_sent(mut answer: Value, input: &str) -> Value {
    if let Some(object) = answer.as_object_mut()
        && !object.contains_key("error")
    {
        let sent = format!("sha256:{}", agent::hex(&Sha256::digest(input.as_bytes())));
        object.insert("sent".into(), Value::String(sent));
    }
    answer
}

/// 请求带工具的：登记总台、把 MCP 条目写进 agy 的配置。出了错的交回答（`unclassified`，原话说明怎么办）；不带工具的交 `None`。
fn wire(job: &Job<'_>, home: &Path) -> Result<Option<Wiring>, Value> {
    if job.declared.is_empty() {
        return Ok(None);
    }
    let wiring = job
        .tools
        .wire(job.declared.clone())
        .map_err(|why| turn::failed(Class::Unclassified, &why))?;
    let program = std::env::current_exe().map_err(|error| {
        turn::failed(
            Class::Unclassified,
            &format!("could not find the path of miyu-antigravity itself, which agy must start as its tool server: {error}"),
        )
    })?;
    let registration = Registration {
        program,
        timeout_seconds: job.config.tool_wait.as_secs() + 60,
        tools: job.declared.iter().map(|tool| tool.name.clone()).collect(),
    };
    mcp_config::register(&mcp_config::path(home), &registration).map_err(|error| {
        turn::failed(
            Class::Unclassified,
            &format!("could not register Miyu's tool server in agy's MCP configuration ({error}); fix or remove that file and try again"),
        )
    })?;
    Ok(Some(wiring))
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
    let wiring = match wire(job, &home) {
        Ok(wiring) => wiring,
        Err(answer) => return answer,
    };
    let work = match workspace::state_dir().and_then(|state| workspace::create_work_dir(&state)) {
        Ok(work) => work,
        Err(error) => {
            return turn::failed(
                Class::Unclassified,
                &format!("could not make a working folder for agy: {error}"),
            );
        }
    };
    let mut live = match Live::start(job.config, job.model, &agent, work.clone(), home, wiring) {
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
    let ended = talk(
        job.out,
        job.call,
        &live,
        &mut turn,
        job.signals,
        job.sender,
        write_input(&live, input.clone()),
    );
    if let Some(conversation) = turn.conversation() {
        live.set_conversation(conversation);
    }
    let handed = matches!(ended, Ended::Handed(..));
    let answer = match ended {
        Ended::Handed(answer, pending) => {
            live.set_pending(pending);
            answer
        }
        other => other.into_answer(),
    };
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
                pending: handed,
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

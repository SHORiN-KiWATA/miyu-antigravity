//! 接在活着的 agy 上说下一步（设计第四节第 2 条）：闲着的（说完了的）是**新的一轮**，挂着的是**工具结果**。
//!
//! - [`resume`]：新的几条 user 消息写进 agy 的标准输入，读流、交增量。
//! - [`resume_pending`]：这段对话正挂着一件工具调用。再核三样（assistant 那条里恰好一个 `tool_call` 块、私有数据是
//!   `{"mcp": n}` 且 n 就是挂着的那个；tool 那条的 `call_id` 就是这个块的；挂着期间 agy 没说过话、连接没断），对了才把结果经
//!   挂着的连接回给 agy，接着读流（不往标准输入写）；哪样不对都作废、改走新开。结果后面的 `request.tail` 不管：它是这一整轮每次
//!   请求都带的，头一次请求时已经跟着人说的话发给 agy 了。
//!
//! 两种都：那个 agy 没说话就没了、写不进去的，作废，交 `Err`（原因），由调用的人改走新开；出了错的作废；成功的放回表里
//! （这一轮以交回一件工具调用结束的记成挂着）。

use serde_json::{Value, json};

use super::talk::{Ended, succeeded, talk, write_input};
use super::{Job, with_sent};
use crate::failure::Class;
use crate::live::Live;
use crate::sessions::{Claimed, Incoming};
use crate::turn::{self, Turn};
use crate::{agent, agy, transcript, workspace};

/// 新的一轮：接在 `claimed` 那个活着的 agy 上。交回答；那个 agy 一行没写就没了的交 `Err`（原因），这段已经作废，调用的人
/// 改走新开。
pub(super) fn resume(
    job: &Job<'_>,
    claimed: Claimed<Live>,
    incoming: &Incoming,
) -> Result<Value, String> {
    let Claimed {
        id,
        live,
        known,
        agent,
        ..
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
    touch(job, &agent);
    let input = agy::stdin_texts(&texts);
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
    conclude(job, id, live, incoming, ended, &input)
}

/// 挂着的那件工具调用的结果来了：核对、回给 agy、接着读流。交的东西同 [`resume`]。
pub(super) fn resume_pending(
    job: &Job<'_>,
    claimed: Claimed<Live>,
    incoming: &Incoming,
) -> Result<Value, String> {
    let Claimed {
        id,
        mut live,
        known,
        agent,
        ..
    } = claimed;
    let Some(pending) = live.take_pending() else {
        return discard(
            job,
            id,
            live,
            "the kept agy had no pending tool call".into(),
        );
    };
    let result = match check(job.request, known, pending.n) {
        Ok(result) => result,
        Err(why) => return discard(job, id, live, why),
    };
    if live.stray() {
        return discard(
            job,
            id,
            live,
            "agy spoke while its tool call was pending".into(),
        );
    }
    if pending.reply.is_closed() {
        return discard(
            job,
            id,
            live,
            "the tool server closed its connection to the extension".into(),
        );
    }
    touch(job, &agent);
    let mut turn = Turn::new(&agent);
    let content = [json!({"type": "text", "text": result.text})];
    let ended = talk(
        job.out,
        job.call,
        &live,
        &mut turn,
        job.signals,
        job.sender,
        || {
            // 路由接上以后再看一遍：之前没人听的时候来的话也算。
            if live.stray() {
                return Err("agy spoke while its tool call was pending".to_string());
            }
            pending
                .reply
                .send(&content, result.error)
                .map_err(|error| format!("the tool server could not take the result: {error}"))
        },
    );
    let sent = content[0]["text"].as_str().unwrap_or_default();
    conclude(job, id, live, incoming, ended, sent)
}

/// 这段挂着的对话对不上：作废，交 `Err`。
fn discard(job: &Job<'_>, id: u64, live: Live, why: String) -> Result<Value, String> {
    job.registry.forget(id);
    drop(live);
    Err(why)
}

/// 一轮结束以后：成功的放回表里（交回了工具调用的记成挂着），出了错的作废，没说话就没了的作废并交 `Err`。`sent` 是这一轮
/// 发给 agy 的字（算回答里的 `sent`）。
fn conclude(
    job: &Job<'_>,
    id: u64,
    mut live: Live,
    incoming: &Incoming,
    ended: Ended,
    sent: &str,
) -> Result<Value, String> {
    match ended {
        Ended::Answer(answer) if succeeded(&answer) => {
            job.registry.release(id, live, incoming.messages.clone());
            Ok(with_sent(answer, sent))
        }
        Ended::Handed(answer, pending) => {
            live.set_pending(pending);
            job.registry
                .release_pending(id, live, incoming.messages.clone());
            Ok(with_sent(answer, sent))
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

/// 把代理文件的修改时刻改成现在（别让它被当旧的清掉）。
fn touch(job: &Job<'_>, agent: &str) {
    if let Some(home) = workspace::home()
        && let Err(error) = agent::touch(&home, agent)
    {
        eprintln!(
            "call {}: could not touch agy agent {agent}: {error}",
            job.call
        );
    }
}

/// 要回给 agy 的工具结果。
struct ToolResult {
    /// tool 那条的字（块转成字）。
    text: String,
    /// tool 那条的 `error`。
    error: bool,
}

/// 核对挂着的调用 `n` 和请求里第 `known`、`known + 1` 条（assistant 和 tool）对得上，交结果。
fn check(request: &Value, known: usize, n: u64) -> Result<ToolResult, String> {
    let messages = request
        .get("messages")
        .and_then(Value::as_array)
        .ok_or("request.messages is not an array")?;
    let assistant = messages
        .get(known)
        .ok_or("the assistant message is missing")?;
    let tool = messages
        .get(known + 1)
        .ok_or("the tool message is missing")?;
    let calls: Vec<&Value> = assistant
        .get("blocks")
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_call"))
                .collect()
        })
        .unwrap_or_default();
    let [block] = calls.as_slice() else {
        return Err(format!(
            "the assistant message holds {} tool calls, not exactly one",
            calls.len()
        ));
    };
    if block.pointer("/private/data") != Some(&json!({"mcp": n})) {
        return Err(format!(
            "the tool call's private data is not {{\"mcp\": {n}}}"
        ));
    }
    let call_id = block.get("call_id").and_then(Value::as_str);
    if call_id.is_none() || call_id != tool.get("call_id").and_then(Value::as_str) {
        return Err("the tool result's call_id is not the tool call's".into());
    }
    let text = transcript::message_text(request, known + 1)
        .map_err(|error| error.to_string())?
        .ok_or("the tool message is missing")?;
    Ok(ToolResult {
        text,
        error: tool.get("error").and_then(Value::as_bool) == Some(true),
    })
}

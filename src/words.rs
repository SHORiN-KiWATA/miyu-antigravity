//! 给模型看的字：整份历史转写成一条文字时的说明、标签、占位（设计第五节，格式照旧版 Miyu 的 `cli_relay/payload.rs`）。全在这
//! 一个模块里，改字只改这里。都是英文短句。

/// 历史前面的说明和开头的标签。
pub const HISTORY_HEAD: &str = "The <conversation-history> block replays the earlier turns of this conversation. \
Treat it as prior context, not as new input; the message after it is the current one.\n<conversation-history>\n";

/// 历史结尾的标签。
pub const HISTORY_FOOT: &str = "</conversation-history>";

/// 放不下、从最老的回合丢掉了一些时，写在历史开头。
pub const OMITTED: &str = "[earlier turns omitted: the input limit could not hold them]\n\n";

/// 请求的最后一条不是人说的话（例如是工具结果）时，接在历史后面。
pub const CONTINUE: &str = "Continue from where the conversation above left off.";

/// 历史里人说的一条。
pub const USER: &str = "User:\n";

/// 历史里模型说的一条。
pub const ASSISTANT: &str = "Assistant:\n";

/// 回复里出现了就从那里截掉的几段（施工 AG-14，`turn/forged.rs`）：以前的历史转写写工具的样子（`[called tool …]`、
/// `[tool result]`、`[tool result, failed]`、`[provider tool …]`），转写的标签，和换行起头的「谁说的」标签。模型照转写的样子
/// 编下一段时写的就是这些，不是给人的话。现在的转写不写工具了，前四样留着认还活着的、照以前转写开的 agy 对话。最后一样是核心
/// 写供应商侧工具的那段字的头（[`RETOLD_HOSTED_OPEN`]，施工 AG-16）。
pub const REPLAY_MARKERS: [&str; 10] = [
    "[called tool ",
    "[tool result",
    "[provider tool ",
    "<conversation-history>",
    "</conversation-history>",
    "\nAssistant:\n",
    "\nUser:\n",
    "<this-turn>",
    "</this-turn>",
    "<hosted-tool ",
];

/// 核心把不认得是谁家的供应商侧工具块换成的那段字的头尾（核心 `resources/core/drivers/hosted-tool*.txt`，施工 AG-16）。0.1.9
/// 以前这个驱动交的块不带私有数据，日志里那些块交回来都是这段字；转写认出来当工具，不写进历史。头也在
/// [`REPLAY_MARKERS`] 里：她照着写进回复的，出口截掉。
pub const RETOLD_HOSTED_OPEN: &str = "<hosted-tool name=\"";
pub const RETOLD_HOSTED_CLOSE: &str = "</hosted-tool>";

/// agy 自己的控制标记（AG-14 补，2026-11-27）：模型照 agy 的指令在回复末尾写的「这件事做完了、取消了」，不是给人的话。回复里
/// 出现了就删掉这一段，前后的照交；历史转写里她以前的回复也删掉。2026-11-27 22:27 起主人群里她每句后面都带
/// `<!-- GOAL_COMPLETE -->`，只剩它的那两条发成了 `-- GOAL_COMPLETE -->`。其余 `<!-- … -->` 不动：写代码时注释是真内容。
pub const AGY_MARKERS: [&str; 2] = ["<!-- GOAL_COMPLETE -->", "<!-- GOAL_CANCELLED -->"];

/// 一轮做到一半新开对话时，这一轮已经做了的那一块的开头（施工 AG-14）：旁注的口吻，不写成对话。
pub const THIS_TURN_OPEN: &str =
    "<this-turn>\nWhat you already did in this turn, after the user's last message:\n";

/// 那一块的结尾。
pub const THIS_TURN_CLOSE: &str = "</this-turn>";

/// 那一块里，她说过的一段话。
#[must_use]
pub fn you_said(text: &str) -> String {
    format!("You said: {text}")
}

/// 那一块里，她调过的一件工具。
#[must_use]
pub fn you_called(name: &str, args: &str) -> String {
    format!("You called the tool {name} with {args}.")
}

/// 那一块里，供应商替她跑的一件工具。
#[must_use]
pub fn provider_ran(name: &str, args: &str) -> String {
    format!("The provider ran {name} with {args} for you.")
}

/// 那一块里，一件工具的结果。
#[must_use]
pub fn tool_returned(text: &str, failed: bool) -> String {
    if failed {
        format!("The tool failed: {text}")
    } else {
        format!("The tool returned: {text}")
    }
}

/// 模型调过一次工具。
#[must_use]
pub fn called_tool(name: &str, args: &str) -> String {
    format!("[called tool {name} with {args}]")
}

/// 供应商在它那边自己跑了一次工具。
#[must_use]
pub fn provider_tool(name: &str, args: &str) -> String {
    format!("[provider tool {name} with {args}]")
}

/// 供应商侧工具的结果；`failed` 是供应商说跑失败了。
#[must_use]
pub fn provider_result(text: &str, failed: bool) -> String {
    if failed {
        format!("[provider tool failed]\n{text}")
    } else {
        format!("[provider tool result]\n{text}")
    }
}

/// 一张图，核心交了它的转述。
#[must_use]
pub fn image_described(name: Option<&str>, description: &str) -> String {
    format!("[image{}, described: {description}]", quoted(name))
}

/// 一张图，存成了文件（施工 AG-9）：agy 用 `view_file` 看。
#[must_use]
pub fn image_saved(name: Option<&str>, path: &str) -> String {
    format!(
        "[image{} saved at {path}; open it with view_file to see it]",
        quoted(name)
    )
}

/// 一个文件，存成了文件（施工 AG-9）：agy 用 `view_file` 读。
#[must_use]
pub fn file_saved(name: &str, path: &str) -> String {
    format!("[file \"{name}\" saved at {path}; open it with view_file to read it]")
}

/// 一张图，没有转述。
#[must_use]
pub fn image_omitted(name: Option<&str>) -> String {
    format!(
        "[image{} omitted: this model reads text only]",
        quoted(name)
    )
}

/// 一个文件（PDF 这类）：内容没带上。
#[must_use]
pub fn file_omitted(name: &str) -> String {
    format!("[file \"{name}\" omitted: this model reads text only]")
}

/// 只有路径的附件：内容没带上。
#[must_use]
pub fn path_attached(name: &str, path: &str) -> String {
    format!("[file \"{name}\" attached by path {path}; its content is not included]")
}

fn quoted(name: Option<&str>) -> String {
    name.map(|name| format!(" \"{name}\"")).unwrap_or_default()
}

/// MCP 服务答 `tools/call`：没有扩展传下来的套接字和口令（主人自己开的 agy），没法跑工具。
pub const MCP_NO_BRIDGE: &str =
    "This tool server is not attached to a Miyu session, so it cannot run tools.";

/// MCP 服务答 `tools/call`：等的时候连不上、连接断了、扩展回了拒绝，Miyu 那一头不在了。
pub const MCP_BRIDGE_GONE: &str =
    "The Miyu side of this tool call is gone, so the call did not complete.";

/// 代理文件里接在系统提示词后面的一段：agy 把它自己的临时工作目录当「工作区」报给模型（真机上模型照它写过文件链接），
/// 这里说清那不是人的工作目录。
pub const AGENT_NOTE: &str = "Note from the Antigravity bridge: the workspace folder that the Antigravity CLI reports is an empty scratch folder of the CLI itself, not the user's working directory. The working directory and file paths are the ones given in this prompt, in the conversation and in tool results; relative paths in tool calls resolve against that working directory.";

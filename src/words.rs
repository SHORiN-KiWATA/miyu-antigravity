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

/// 历史里一次工具的结果。
pub const TOOL_RESULT: &str = "[tool result]\n";

/// 历史里一次工具的结果，工具报了错。
pub const TOOL_FAILED: &str = "[tool result, failed]\n";

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

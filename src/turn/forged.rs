//! 回复里照历史转写的样子编出来的那一截，从第一个标记起截掉（施工 AG-14）。
//!
//! 新开对话时整份历史写成一条字发给 agy（`transcript.rs`）。以前那份转写把她调过的工具写成 `[called tool …]`、`[tool result]`，
//! 模型有时不真调工具，照这个样子写一段字当回复，驱动当正文交给核心，就发进了群里（2026-11-23 起主人的群里 22 次，有一次连
//! `Assistant:` 和下一句话都编了出来）。现在的转写不写工具了（根治），这里是最后一道：回复里出现了 [`words::REPLAY_MARKERS`]
//! 里的一段，那一段和它后面的这一轮都不交，前面的照交。
//!
//! 一边收一边认：标记可能被拆在两条 `text_delta` 里，末尾还可能是标记开头的那几个字先留着，看清了再交。回复的开头当成一行的
//! 开头（`Assistant:` 写在最前面的也认）。截了以后这一轮的字都不交，工具调用照常。

use crate::words::REPLAY_MARKERS;

/// 认标记的状态。
#[derive(Debug, Default)]
pub(super) struct Forged {
    /// 还没交的一截：可能是标记的开头。
    pending: String,
    /// 交过字了：之后的字不在回复开头。
    started: bool,
    /// 截掉了：截掉的那一截的开头（写日志）。
    cut: Option<String>,
    /// 截掉的这一截还没写过日志。
    unlogged: bool,
}

impl Forged {
    /// 来了一段字：交回现在能交的（可能是空的）。
    pub(super) fn pass(&mut self, text: &str) -> String {
        if self.cut.is_some() {
            return String::new();
        }
        self.pending.push_str(text);
        // 回复的开头当成一行的开头：前面垫一个换行再找，交出去的字里不带它。
        let lead = usize::from(!self.started);
        let hay = if self.started {
            self.pending.clone()
        } else {
            format!("\n{}", self.pending)
        };
        if let Some(at) = REPLAY_MARKERS
            .iter()
            .filter_map(|marker| hay.find(marker))
            .min()
        {
            let at = at.saturating_sub(lead).min(self.pending.len());
            let kept = self.pending[..at].trim_end().to_string();
            self.cut = Some(self.pending[at..].chars().take(80).collect());
            self.unlogged = true;
            self.pending.clear();
            self.started |= !kept.is_empty();
            return kept;
        }
        let hold = held_back(&hay).saturating_sub(lead);
        let split = floor_boundary(
            &self.pending,
            self.pending.len() - hold.min(self.pending.len()),
        );
        let out = self.pending[..split].to_string();
        self.pending.drain(..split);
        self.started |= !out.is_empty();
        out
    }

    /// 这一块收了、流完了：留着的交出去（截过的不交）。
    pub(super) fn drain(&mut self) -> String {
        if self.cut.is_some() {
            self.pending.clear();
            return String::new();
        }
        self.started |= !self.pending.is_empty();
        std::mem::take(&mut self.pending)
    }

    /// 刚截掉、还没写过日志的那一截的开头；拿过一次就没了。
    pub(super) fn take_cut(&mut self) -> Option<String> {
        if !self.unlogged {
            return None;
        }
        self.unlogged = false;
        self.cut.clone()
    }
}

/// 一整段字从第一个标记起截掉以后剩的（去掉末尾空白）；没有标记的原样。历史转写里她以前的回复照它（施工 AG-14）：以前漏进群里的
/// 那几条留在了历史里，照原样重放又是一份照抄的样子。回复的开头当成一行的开头，同 [`Forged::pass`]。
pub(crate) fn before_marker(text: &str) -> &str {
    let hay = format!("\n{text}");
    match REPLAY_MARKERS
        .iter()
        .filter_map(|marker| hay.find(marker))
        .min()
    {
        Some(at) => text[..at.saturating_sub(1).min(text.len())].trim_end(),
        None => text,
    }
}

/// `text` 的末尾有多少字节可能是一个标记的开头（标记本身没写全）。
fn held_back(text: &str) -> usize {
    REPLAY_MARKERS
        .iter()
        .flat_map(|marker| {
            (1..marker.len())
                .filter(|&n| marker.is_char_boundary(n))
                .filter(|&n| text.ends_with(&marker[..n]))
        })
        .max()
        .unwrap_or(0)
}

/// 不超过 `at` 的、落在字的边界上的位置。
fn floor_boundary(text: &str, mut at: usize) -> usize {
    while at > 0 && !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

#[cfg(test)]
mod tests;

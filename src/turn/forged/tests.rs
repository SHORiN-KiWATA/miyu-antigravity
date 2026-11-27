//! 认照转写编出来的那一截（施工 AG-14）：从标记起截掉，前面的照交；标记拆在几段里也认；可能是标记开头的末尾先留着，看清了
//! 不是再交；回复开头的 `Assistant:` 也认；截了以后的字都不交；没有标记的逐字照交。

use super::{Forged, before_marker};

/// 一段段喂进去，交出来的接在一起，最后收块。
fn through(pieces: &[&str]) -> (String, Option<String>) {
    let mut forged = Forged::default();
    let mut out = String::new();
    for piece in pieces {
        out.push_str(&forged.pass(piece));
    }
    out.push_str(&forged.drain());
    (out, forged.take_cut())
}

#[test]
fn a_reply_without_markers_passes_whole() {
    let (out, cut) = through(&["你好", "，今天", "吃了吗 [笑]"]);
    assert_eq!(out, "你好，今天吃了吗 [笑]");
    assert_eq!(cut, None);
}

#[test]
fn a_fake_tool_call_at_the_start_leaves_nothing() {
    // 2026-11-27 16:02 主人的群里发出去的原样。
    let (out, cut) = through(&[
        "[called tool qq_skip_reply with {\"reason\":\"群友在闲聊\"}]\n\n[tool result]\nSkipped: nothing in this reply was sent.\n",
    ]);
    assert_eq!(out, "");
    assert!(cut.unwrap().starts_with("[called tool qq_skip_reply"));
}

#[test]
fn what_she_said_before_the_marker_still_goes_out() {
    let (out, _) = through(&[
        "在窥屏呢\n\n",
        "[tool result]\nYour next message will quote it.",
    ]);
    // 标记前面的换行可能已经交了，不要紧。
    assert_eq!(out.trim_end(), "在窥屏呢");
}

#[test]
fn a_marker_split_across_pieces_is_still_caught() {
    let (out, cut) = through(&["好的", "[cal", "led to", "ol qq_quote with {}]", "后面的"]);
    assert_eq!(out, "好的");
    assert!(cut.is_some());
}

#[test]
fn a_bracket_that_turns_out_not_to_be_a_marker_is_released() {
    let (out, cut) = through(&["看这个[", "图]"]);
    assert_eq!(out, "看这个[图]");
    assert_eq!(cut, None);
}

#[test]
fn an_assistant_label_counts_at_the_start_and_after_a_newline_only() {
    let (out, cut) = through(&["Assistant:\n在窥屏呢"]);
    assert_eq!((out.as_str(), cut.is_some()), ("", true));
    let (out, cut) = through(&["好\nAssistant:\n再编一句"]);
    assert_eq!((out.as_str(), cut.is_some()), ("好", true));
    // 不在一行开头的不算。
    let (out, cut) = through(&["the Assistant:\nline"]);
    assert_eq!((out.as_str(), cut), ("the Assistant:\nline", None));
}

#[test]
fn nothing_after_a_cut_goes_out_and_it_is_logged_once() {
    let mut forged = Forged::default();
    assert_eq!(forged.pass("前面"), "前面");
    assert_eq!(forged.pass("\nUser:\n编的"), "");
    assert_eq!(forged.pass("还在编"), "");
    assert_eq!(forged.drain(), "");
    assert!(forged.take_cut().is_some());
    assert_eq!(forged.take_cut(), None);
}

#[test]
fn a_whole_old_reply_is_cut_at_its_first_marker() {
    assert_eq!(
        before_marker(
            "[called tool qq_quote with {\"msg\":\"1\"}]\n\n[tool result]\nok\n\nAssistant:\n在窥屏呢"
        ),
        ""
    );
    assert_eq!(before_marker("在窥屏呢\n\n[tool result]\nok"), "在窥屏呢");
    assert_eq!(before_marker("Assistant:\n编的"), "");
    assert_eq!(before_marker("正常的一句 [图]"), "正常的一句 [图]");
}

/// agy 自己的控制标记（AG-14 补，根治在 `agy.rs` 关掉斜杠命令，这里是最后一道）：只删标记和它前面的空格，前后的话照交；
/// 标记拆在几段里也删；整条只有它的什么都不交；别的 HTML 注释不动。2026-11-27 主人群里的原样。
#[test]
fn agy_control_markers_are_dropped_and_the_words_kept() {
    let (out, cut) = through(&["你当这是跑agent呢，想得美 <!-- GOAL_COMPLETE -->\n"]);
    assert_eq!((out.as_str(), cut), ("你当这是跑agent呢，想得美\n", None));
    let (out, _) = through(&["美得你 <!-- GOAL", "_COMPL", "ETE -->"]);
    assert_eq!(out, "美得你");
    let (out, _) = through(&["<!-- GOAL_COMPLETE -->"]);
    assert_eq!(out, "");
    let (out, _) = through(&["算了 <!-- GOAL_CANCELLED -->"]);
    assert_eq!(out, "算了");
    let (out, _) = through(&["<!-- 注释 --> 照留"]);
    assert_eq!(out, "<!-- 注释 --> 照留");
    assert_eq!(
        before_marker("突然这么正经 <!-- GOAL_COMPLETE -->\n"),
        "突然这么正经"
    );
}

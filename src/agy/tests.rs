use std::path::PathBuf;

use super::*;

#[test]
fn chat_args_are_the_design_line_word_for_word() {
    let args = chat_args(
        "gemini-3.8-flash-low",
        "miyu-0123456789abcdef",
        Path::new("/state/work/7-1"),
    );
    let args: Vec<&str> = args.iter().map(|arg| arg.to_str().unwrap()).collect();
    assert_eq!(
        args,
        [
            "--print=",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--dangerously-skip-permissions",
            "--disable-slash-commands",
            "--model",
            "gemini-3.8-flash-low",
            "--agent",
            "miyu-0123456789abcdef",
            "--add-dir",
            "/state/work/7-1",
            "--print-timeout",
            "86400s",
        ]
    );
}

#[test]
fn stdin_line_is_one_user_event() {
    let line = stdin_line("hi \"there\"\nsecond line");
    assert!(line.ends_with('\n'));
    assert_eq!(line.matches('\n').count(), 1, "整条一行");
    let value: serde_json::Value = serde_json::from_str(line.trim_end()).unwrap();
    assert_eq!(
        value,
        json!({"event": "user", "message": {"content": [{"type": "text", "text": "hi \"there\"\nsecond line"}]}})
    );
}

#[test]
fn stdin_texts_puts_one_text_block_per_message() {
    let line = stdin_texts(&["first", "second\nline"]);
    assert_eq!(line.matches('\n').count(), 1, "整条一行");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(line.trim_end()).unwrap(),
        json!({"event": "user", "message": {"content": [
            {"type": "text", "text": "first"},
            {"type": "text", "text": "second\nline"}
        ]}})
    );
}

#[test]
fn command_uses_the_configured_program_and_leaves_home_alone() {
    let plain = command(&Config::default(), MODELS_ARGS);
    assert_eq!(plain.get_program(), "agy");
    assert_eq!(plain.get_envs().count(), 0, "环境照扩展自己的原样传");

    let config = Config {
        binary: Some(PathBuf::from("/opt/agy/bin/agy")),
        ..Config::default()
    };
    let set = command(&config, MODELS_ARGS);
    assert_eq!(set.get_program(), "/opt/agy/bin/agy");
    assert_eq!(set.get_args().collect::<Vec<_>>(), ["models"]);
    assert_eq!(set.get_envs().count(), 0);
}

/// 上限算在字上（几段加起来）：正好 192,000 字节的不算超，多一个字节的交总数。
#[test]
fn over_limit_counts_the_bytes_of_the_texts_together() {
    assert_eq!(TEXT_LIMIT, 192_000);
    let at = "a".repeat(TEXT_LIMIT);
    assert_eq!(over_limit(&[at.as_str()]), None);
    assert_eq!(over_limit(&[at.as_str(), "b"]), Some(TEXT_LIMIT + 1));
    // 全是要转义的字：写进标准输入的那一行比上限长得多，字本身没超的不算超。
    let quotes = "\"q\"\n".repeat(42_000);
    assert!(stdin_line(&quotes).len() > TEXT_LIMIT);
    assert_eq!(over_limit(&[quotes.as_str()]), None);
    assert_eq!(over_limit(&["汉".repeat(64_001).as_str()]), Some(192_003));
    let nothing: [&str; 0] = [];
    assert_eq!(over_limit(&nothing), None);
}

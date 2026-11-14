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

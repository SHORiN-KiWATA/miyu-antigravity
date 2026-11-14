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
fn command_uses_the_configured_program_and_home() {
    let plain = command(&Config::default(), MODELS_ARGS);
    assert_eq!(plain.get_program(), "agy");
    assert!(plain.get_envs().all(|(key, _)| key != "HOME"));

    let config = Config {
        binary: Some(PathBuf::from("/opt/agy/bin/agy")),
        home: Some(PathBuf::from("/accounts/work")),
    };
    let set = command(&config, MODELS_ARGS);
    assert_eq!(set.get_program(), "/opt/agy/bin/agy");
    assert_eq!(set.get_args().collect::<Vec<_>>(), ["models"]);
    let home: Vec<_> = set.get_envs().filter(|(key, _)| *key == "HOME").collect();
    assert_eq!(
        home,
        [(
            std::ffi::OsStr::new("HOME"),
            Some(std::ffi::OsStr::new("/accounts/work"))
        )]
    );
}

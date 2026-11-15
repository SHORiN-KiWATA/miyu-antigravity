use std::io::Cursor;

use super::*;

const TRANSCRIPT: &str = include_str!("../../tests/samples/real-search-read.transcript.jsonl");

/// 样本里第 `step` 步的 `content` 原文。
fn raw_content(step: u64) -> String {
    TRANSCRIPT
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .find(|value| value["step_index"] == step)
        .unwrap()["content"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn the_real_transcript_gives_the_search_result_without_the_two_time_lines() {
    let found = result_in(Cursor::new(TRANSCRIPT), 2).unwrap().unwrap();
    assert!(
        found.starts_with("The search for \"EditorConfig specification\" returned"),
        "{found:?}"
    );
    assert!(!found.contains("Created At:") && !found.contains("Completed At:"));
    // 去掉的恰好是两行：其余原样。
    let raw = raw_content(2);
    let lines: Vec<&str> = raw.splitn(3, '\n').collect();
    assert!(lines[0].starts_with("Created At: 2026-11-14T21:19:37"));
    assert!(lines[1].starts_with("Completed At: 2026-11-14T21:19:43"));
    assert_eq!(found, lines[2]);

    let page = result_in(Cursor::new(TRANSCRIPT), 4).unwrap().unwrap();
    assert!(
        page.starts_with("Title: EditorConfig Specification"),
        "{page:?}"
    );
}

#[test]
fn a_step_that_is_not_there_or_has_no_words_is_none() {
    // 没有这一步；有这一步但没有 content（模型调用那一行）。
    assert_eq!(result_in(Cursor::new(TRANSCRIPT), 99).unwrap(), None);
    assert_eq!(result_in(Cursor::new(TRANSCRIPT), 3).unwrap(), None);
    assert_eq!(result_in(Cursor::new(""), 2).unwrap(), None);
    // 只有两行时间、后面没有字的，也当没有。
    let only_times = r#"{"step_index":2,"content":"Created At: a\nCompleted At: b\n"}"#;
    assert_eq!(result_in(Cursor::new(only_times), 2).unwrap(), None);
}

#[test]
fn content_without_the_two_time_lines_is_kept_as_it_is() {
    assert_eq!(strip_times("plain words"), "plain words");
    assert_eq!(strip_times("Created At: x\nplain"), "Created At: x\nplain");
    assert_eq!(
        strip_times("Created At: x\nCompleted At: y\nrest\nmore"),
        "rest\nmore"
    );
    assert_eq!(
        strip_times("Other\nCompleted At: y\nrest"),
        "Other\nCompleted At: y\nrest"
    );
    let line = r#"{"step_index":7,"content":"just a title"}"#;
    assert_eq!(result_of(line, 7).as_deref(), Some("just a title"));
}

#[test]
fn bad_lines_are_skipped_and_the_last_row_with_words_wins() {
    let text = [
        "this is not json",
        r#"{"step_index":5,"content":"first"}"#,
        r#"{"step_index":"5","content":"wrong type"}"#,
        r#"{"no":"index"}"#,
        r#"{"step_index":5,"content":7}"#,
        "",
        r#"{"step_index":5,"content":"second"}"#,
        r#"{"step_index":6,"content":"other step"}"#,
    ]
    .join("\n");
    assert_eq!(
        result_in(Cursor::new(text), 5).unwrap().as_deref(),
        Some("second")
    );
}

#[test]
fn a_line_over_the_limit_is_skipped_without_reading_it_whole() {
    // 上限 64 字节：中间那一行有 1000 字节，跳过；前后的行照读，包括没有结尾换行的最后一行。
    let long = format!(r#"{{"step_index":2,"content":"{}"}}"#, "x".repeat(1000));
    let text = format!(
        "{}\n{long}\n{}",
        r#"{"step_index":1,"content":"a"}"#, r#"{"step_index":3,"content":"c"}"#
    );
    let at = |step| result_within(Cursor::new(text.clone()), step, 64).unwrap();
    assert_eq!(at(1).as_deref(), Some("a"));
    assert_eq!(at(2), None, "太长的那一行不读");
    assert_eq!(at(3).as_deref(), Some("c"));
}

#[test]
fn the_path_follows_agys_layout_and_refuses_odd_conversation_ids() {
    let home = Path::new("/h");
    assert_eq!(
        path(home, "1cdae2e2-2ada").unwrap(),
        Path::new(
            "/h/.gemini/antigravity-cli/brain/1cdae2e2-2ada/.system_generated/logs/transcript_full.jsonl"
        )
    );
    for odd in ["", "../x", "a/b", "a b"] {
        assert!(path(home, odd).is_none(), "{odd:?}");
        assert_eq!(read(home, odd, 1), None);
    }
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "miyu-antigravity-brain-{name}-{}",
        workspace::unique()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn read_finds_the_file_under_home_and_gives_none_without_it() {
    let home = scratch("read");
    assert_eq!(read(&home, "c-1", 2), None, "还没有转录文件");
    let file = path(&home, "c-1").unwrap();
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, TRANSCRIPT).unwrap();
    let found = read(&home, "c-1", 4).unwrap();
    assert!(found.starts_with("Title: EditorConfig"));
    assert_eq!(read(&home, "c-1", 3), None);
    // 只读：文件原样还在。
    assert_eq!(std::fs::read_to_string(&file).unwrap(), TRANSCRIPT);
    std::fs::remove_dir_all(&home).unwrap();
}

#[test]
fn read_waiting_sees_a_row_that_arrives_late_and_gives_up_at_the_deadline() {
    let home = scratch("waiting");
    let file = path(&home, "c-2").unwrap();
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    // 晚到：另一个线程隔一会儿再写。
    let writer = {
        let file = file.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            std::fs::write(&file, "{\"step_index\":2,\"content\":\"late words\"}\n").unwrap();
        })
    };
    let found = read_waiting(
        &home,
        "c-2",
        2,
        Duration::from_secs(20),
        Duration::from_millis(10),
    );
    writer.join().unwrap();
    assert_eq!(found.as_deref(), Some("late words"));

    // 一直没有：到点就算读不到（等的上限给得很短，只看交 None）。
    let started = Instant::now();
    assert_eq!(
        read_waiting(
            &home,
            "c-2",
            9,
            Duration::from_millis(100),
            Duration::from_millis(10)
        ),
        None
    );
    assert!(started.elapsed() >= Duration::from_millis(100));
    std::fs::remove_dir_all(&home).unwrap();
}

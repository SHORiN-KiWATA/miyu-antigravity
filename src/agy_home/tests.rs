use super::*;

fn scratch(tag: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("miyu-antigravity-ah-{tag}-{}", workspace::unique()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn mark_file(state: &Path, conversation: &str) -> PathBuf {
    leftovers::kept_dir(state).join(conversation)
}

fn pid_line() -> String {
    format!("{}\n", std::process::id())
}

#[test]
fn without_a_report_the_own_home_is_used_and_the_mark_has_one_line() {
    let (own, state) = (scratch("own"), scratch("state"));
    let home = AgyHome::new(own.clone(), state.clone(), 7, &Arc::default());
    assert_eq!(home.current(), own);
    home.note_conversation("abc-1");
    assert_eq!(
        std::fs::read_to_string(mark_file(&state, "abc-1")).unwrap(),
        pid_line()
    );
}

#[test]
fn a_reported_home_is_used_and_a_report_equal_to_the_own_home_adds_no_second_line() {
    let (own, state) = (scratch("own2"), scratch("state2"));
    let reported = Arc::new(Reported::default());
    let home = AgyHome::new(own.clone(), state.clone(), 7, &reported);
    reported.report(&own);
    assert_eq!(home.current(), own);
    home.note_conversation("abc-1");
    assert_eq!(
        std::fs::read_to_string(mark_file(&state, "abc-1")).unwrap(),
        pid_line()
    );
    reported.report(Path::new("/fake/agy-home"));
    assert_eq!(home.current(), PathBuf::from("/fake/agy-home"));
}

#[test]
fn a_mark_written_before_the_report_is_rewritten_after_it_and_keeps_the_pid() {
    let (own, state) = (scratch("own3"), scratch("state3"));
    let reported = Arc::new(Reported::default());
    let home = AgyHome::new(own, state.clone(), 7, &reported);
    home.note_conversation("abc-1");
    let file = mark_file(&state, "abc-1");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), pid_line());
    reported.report(Path::new("/fake/agy-home"));
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        format!("{}/fake/agy-home\n", pid_line())
    );
    // 同一个值再报一次：不重写（内容照旧）。
    reported.report(Path::new("/fake/agy-home"));
    assert_eq!(
        std::fs::read_dir(leftovers::kept_dir(&state))
            .unwrap()
            .count(),
        1
    );
}

#[test]
fn a_mark_written_after_the_report_has_both_lines_at_once() {
    let (own, state) = (scratch("own4"), scratch("state4"));
    let reported = Arc::new(Reported::default());
    let home = AgyHome::new(own, state.clone(), 7, &reported);
    reported.report(Path::new("/fake/agy-home"));
    home.note_conversation("abc-1");
    assert_eq!(
        std::fs::read_to_string(mark_file(&state, "abc-1")).unwrap(),
        format!("{}/fake/agy-home\n", pid_line())
    );
}

#[test]
fn a_new_conversation_replaces_the_old_mark_and_finish_removes_it_only_when_clean() {
    let (own, state) = (scratch("own5"), scratch("state5"));
    let home = AgyHome::new(own, state.clone(), 7, &Arc::default());
    home.note_conversation("a-1");
    home.note_conversation("a-1");
    home.note_conversation("b-2");
    assert!(!mark_file(&state, "a-1").exists());
    assert!(mark_file(&state, "b-2").exists());
    home.note_conversation("../x");
    assert!(!mark_file(&state, "../x").exists());
    home.finish(false);
    assert!(mark_file(&state, "b-2").exists(), "没删干净的记号留着");
    home.note_conversation("c-3");
    home.finish(true);
    assert!(!mark_file(&state, "c-3").exists());
}

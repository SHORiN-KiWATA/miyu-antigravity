use super::*;

const ME: u32 = 100;

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(ToString::to_string).collect()
}

/// 进程 200 在，别的都不在。
fn only_200(pid: u32) -> bool {
    pid == 200
}

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "miyu-antigravity-left-{tag}-{}",
        workspace::unique()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn pids_come_out_of_names_of_the_right_shape_only() {
    assert_eq!(pid_of_socket("antigravity-123.sock"), Some(123));
    for odd in [
        "antigravity-.sock",
        "antigravity-12a.sock",
        "antigravity-123.sock.bak",
        "antigravity-0.sock",
        "antigravity-99999999999.sock",
        "antigravity--5.sock",
        "other-123.sock",
        "123.sock",
    ] {
        assert_eq!(pid_of_socket(odd), None, "{odd}");
    }
    assert_eq!(pid_of_work("123-4567-0"), Some(123));
    for odd in ["123", "abc-1", "-1-2", "0-1-2", "12x-1", ""] {
        assert_eq!(pid_of_work(odd), None, "{odd}");
    }
}

#[test]
fn sockets_of_dead_processes_go() {
    let all = names(&["antigravity-300.sock"]);
    assert_eq!(dead_sockets(&all, ME, only_200), ["antigravity-300.sock"]);
}

#[test]
fn sockets_of_ourselves_and_of_living_processes_stay() {
    let all = names(&["antigravity-100.sock", "antigravity-200.sock"]);
    assert!(dead_sockets(&all, ME, |_| false).len() == 1, "自己的不清");
    assert!(dead_sockets(&all, ME, only_200).is_empty());
}

#[test]
fn socket_names_of_other_shapes_are_left_alone() {
    let all = names(&[
        "notes.txt",
        "antigravity-x.sock",
        "antigravity-300.sock.tmp",
        "bridge-300.sock",
    ]);
    assert!(dead_sockets(&all, ME, |_| false).is_empty());
}

#[test]
fn work_dirs_follow_the_same_rule() {
    let all = names(&["300-1-0", "200-1-0", "100-1-0", "scratch", "300"]);
    assert_eq!(dead_work(&all, ME, only_200), ["300-1-0"]);
}

#[test]
fn marks_of_dead_processes_clean_their_conversation() {
    let marks = vec![
        ("conv-a".to_string(), "300\n".to_string()),
        ("conv-b".to_string(), "200\n".to_string()),
        ("conv-c".to_string(), "100\n".to_string()),
    ];
    assert_eq!(
        dead_marks(&marks, ME, only_200),
        [Verdict::Conversation("conv-a".to_string())]
    );
}

#[test]
fn marks_with_unreadable_content_lose_only_the_mark() {
    let marks = vec![
        ("conv-a".to_string(), "not a number".to_string()),
        ("conv-b".to_string(), String::new()),
        ("conv-c".to_string(), "0\n".to_string()),
        ("conv-d".to_string(), "12 34".to_string()),
    ];
    let verdicts = dead_marks(&marks, ME, |_| true);
    assert_eq!(verdicts.len(), 4);
    assert!(
        verdicts
            .iter()
            .all(|verdict| matches!(verdict, Verdict::Unreadable(_)))
    );
}

#[test]
fn marks_with_odd_names_are_not_touched() {
    let marks = vec![
        (".conv.tmp".to_string(), "300\n".to_string()),
        ("a.b".to_string(), "xx".to_string()),
        ("..".to_string(), "300".to_string()),
    ];
    assert!(dead_marks(&marks, ME, |_| false).is_empty());
}

#[test]
fn our_own_process_is_alive_and_a_reaped_one_is_not() {
    assert!(process_alive(std::process::id()));
    let mut child = std::process::Command::new("true").spawn().unwrap();
    let pid = child.id();
    child.wait().unwrap();
    assert!(!process_alive(pid));
}

#[test]
fn a_mark_holds_our_pid_and_goes_away() {
    let state = scratch("mark");
    mark(&state, "abc-1").unwrap();
    let file = kept_dir(&state).join("abc-1");
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        format!("{}\n", std::process::id())
    );
    let left: Vec<_> = std::fs::read_dir(kept_dir(&state)).unwrap().collect();
    assert_eq!(left.len(), 1, "没有留下临时文件");
    mark(&state, "abc-1").unwrap();
    unmark(&state, "abc-1").unwrap();
    assert!(!file.exists());
    unmark(&state, "abc-1").unwrap();
    std::fs::remove_dir_all(&state).unwrap();
}

#[test]
fn odd_conversation_ids_get_no_mark() {
    let state = scratch("odd");
    for id in ["", "..", "a/b", "a.b", "../x"] {
        assert!(mark(&state, id).is_err(), "{id:?}");
    }
    assert!(
        !kept_dir(&state).exists() || std::fs::read_dir(kept_dir(&state)).unwrap().count() == 0
    );
    std::fs::remove_dir_all(&state).unwrap();
}

#[test]
fn sweeping_removes_what_a_dead_process_left_and_nothing_else() {
    let state = scratch("sweep-state");
    let runtime = scratch("sweep-runtime");
    let home = scratch("sweep-home");
    let me = std::process::id();
    let mut child = std::process::Command::new("true").spawn().unwrap();
    let dead = child.id();
    child.wait().unwrap();
    let conversations = home.join(".gemini/antigravity-cli/conversations");
    std::fs::create_dir_all(&conversations).unwrap();
    std::fs::create_dir_all(home.join(".gemini/antigravity-cli/brain/gone-1")).unwrap();
    for name in ["gone-1.db", "mine-1.db"] {
        std::fs::write(conversations.join(name), "x").unwrap();
    }
    std::fs::write(runtime.join(format!("antigravity-{dead}.sock")), "").unwrap();
    std::fs::write(runtime.join(format!("antigravity-{me}.sock")), "").unwrap();
    std::fs::write(runtime.join("antigravity-abc.sock"), "").unwrap();
    for name in [
        format!("{dead}-1-0"),
        format!("{me}-1-0"),
        "plain".to_string(),
    ] {
        std::fs::create_dir_all(state.join("work").join(name)).unwrap();
    }
    std::fs::create_dir_all(kept_dir(&state)).unwrap();
    std::fs::write(kept_dir(&state).join("gone-1"), format!("{dead}\n")).unwrap();
    std::fs::write(kept_dir(&state).join("mine-1"), format!("{me}\n")).unwrap();

    sweep(&state, Some(&runtime), Some(&home));

    let ls = |dir: &Path| {
        let mut all = list(dir).unwrap();
        all.sort();
        all
    };
    let mut kept_sockets = vec![
        format!("antigravity-{me}.sock"),
        "antigravity-abc.sock".to_string(),
    ];
    kept_sockets.sort();
    assert_eq!(ls(&runtime), kept_sockets);
    let mut kept_work = vec![format!("{me}-1-0"), "plain".to_string()];
    kept_work.sort();
    assert_eq!(ls(&state.join("work")), kept_work);
    assert_eq!(ls(&kept_dir(&state)), ["mine-1"]);
    assert_eq!(ls(&conversations), ["mine-1.db"]);
    assert!(!home.join(".gemini/antigravity-cli/brain/gone-1").exists());
    for dir in [state, runtime, home] {
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn a_mark_with_a_second_line_names_the_conversations_home() {
    let marks = vec![
        ("conv-a".to_string(), "300\n/fake/home\n".to_string()),
        ("conv-b".to_string(), "200\n/fake/home\n".to_string()),
    ];
    assert_eq!(
        dead_marks(&marks, ME, only_200),
        [Verdict::Conversation("conv-a".to_string())],
        "进程号照第一行认"
    );
    assert_eq!(
        mark_home("300\n/fake/home\n"),
        Some(PathBuf::from("/fake/home"))
    );
    assert_eq!(
        mark_home("300\n/fake/home\nmore\n"),
        Some(PathBuf::from("/fake/home"))
    );
}

#[test]
fn an_old_one_line_mark_or_a_second_line_that_is_not_absolute_has_no_home() {
    for content in [
        "300\n",
        "300",
        "",
        "300\nrelative/dir\n",
        "300\n\n",
        "300\n/half/written",
    ] {
        assert_eq!(mark_home(content), None, "{content:?}");
    }
}

#[test]
fn marks_are_written_with_one_line_or_two_and_rewriting_keeps_the_pid() {
    let state = scratch("mark-two");
    let file = kept_dir(&state).join("abc-1");
    mark(&state, "abc-1").unwrap();
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        format!("{}\n", std::process::id())
    );
    mark_with_home(&state, "abc-1", Some(Path::new("/fake/home"))).unwrap();
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        format!("{}\n/fake/home\n", std::process::id())
    );
    assert_eq!(
        parse_pid(read_small(&file).lines().next().unwrap()),
        Some(std::process::id())
    );
    mark_with_home(&state, "abc-1", None).unwrap();
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        format!("{}\n", std::process::id())
    );
    assert_eq!(
        std::fs::read_dir(kept_dir(&state)).unwrap().count(),
        1,
        "没有临时文件"
    );
    std::fs::remove_dir_all(&state).unwrap();
}

#[test]
fn sweeping_deletes_under_the_marked_home_when_there_is_one_and_the_own_home_otherwise() {
    let (state, own, fake) = (scratch("hs-state"), scratch("hs-own"), scratch("hs-fake"));
    let mut child = std::process::Command::new("true").spawn().unwrap();
    let dead = child.id();
    child.wait().unwrap();
    let plant = |home: &Path, id: &str| {
        let base = home.join(".gemini/antigravity-cli");
        std::fs::create_dir_all(base.join("conversations")).unwrap();
        std::fs::create_dir_all(base.join("brain").join(id)).unwrap();
        std::fs::write(base.join("conversations").join(format!("{id}.db")), "x").unwrap();
    };
    for (home, id) in [
        (&fake, "two-1"),
        (&own, "one-1"),
        (&own, "rel-1"),
        (&fake, "rel-1"),
    ] {
        plant(home, id);
    }
    std::fs::create_dir_all(kept_dir(&state)).unwrap();
    let write =
        |id: &str, content: String| std::fs::write(kept_dir(&state).join(id), content).unwrap();
    write("two-1", format!("{dead}\n{}\n", fake.display()));
    write("one-1", format!("{dead}\n"));
    write("rel-1", format!("{dead}\nrelative\n"));

    sweep(&state, None, Some(&own));

    let exists =
        |home: &Path, id: &str| home.join(".gemini/antigravity-cli/brain").join(id).exists();
    assert!(!exists(&fake, "two-1"), "第二行的家目录下删了");
    assert!(!exists(&own, "one-1"), "一行的老记号照自己的家目录");
    assert!(!exists(&own, "rel-1"), "第二行不是绝对路径的照自己的家目录");
    assert!(exists(&fake, "rel-1"), "不是绝对路径的那行不当路径用");
    assert_eq!(list(&kept_dir(&state)).unwrap().len(), 0, "记号都删了");
    for dir in [state, own, fake] {
        std::fs::remove_dir_all(dir).unwrap();
    }
}

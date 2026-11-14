//! 一次调用在磁盘上留下的：给 agy 进程新建的空工作目录（`MIYU_STATE_DIR/work/<随机>`，`--add-dir` 也给它），和 agy 自己记
//! 下的这段对话（`<家目录>/.gemini/antigravity-cli/conversations/<id>.db*`、`brain/<id>/`）。用完都删（设计第八节「清理」）。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// 一个这个进程里不重、跨进程也几乎不撞的名字：进程号、纳秒、计数。
#[must_use]
pub fn unique() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    format!(
        "{}-{nanos}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

/// 扩展自己的家目录（`HOME`）：agy 照它跑，代理文件、agy 记的对话都在它下面。没设、空的是 `None`。
#[must_use]
pub fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
}

/// 扩展的状态目录：`MIYU_STATE_DIR`，没设的用当前目录（核心拉起时两个是同一处）。
///
/// # Errors
///
/// 都没有：读当前目录的错原样交。
pub fn state_dir() -> std::io::Result<PathBuf> {
    match std::env::var_os("MIYU_STATE_DIR").filter(|dir| !dir.is_empty()) {
        Some(dir) => Ok(PathBuf::from(dir)),
        None => std::env::current_dir(),
    }
}

/// 在 `state` 下新建一个空的工作目录 `work/<随机>`。
///
/// # Errors
///
/// 建不成：原样交。
pub fn create_work_dir(state: &Path) -> std::io::Result<PathBuf> {
    let parent = state.join("work");
    std::fs::create_dir_all(&parent)?;
    let dir = parent.join(unique());
    std::fs::create_dir(&dir)?;
    Ok(dir)
}

/// 删掉工作目录 `dir`；已经没了不算错。
///
/// # Errors
///
/// 删不掉：原样交。
pub fn remove_work_dir(dir: &Path) -> std::io::Result<()> {
    absent_ok(std::fs::remove_dir_all(dir))
}

/// agy 的对话编号只认字母、数字、`-`：别的（空的、带 `/`、`..`）不拿来拼路径。
#[must_use]
pub fn valid_conversation(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// 删掉 `home` 下 agy 记的对话 `id`：`conversations/<id>.db`、`.db-shm`、`.db-wal` 和 `brain/<id>/`；没有的不算错。
///
/// # Errors
///
/// 编号不合 [`valid_conversation`]、删不掉：交第一个错，别的照样删。
pub fn remove_conversation(home: &Path, id: &str) -> std::io::Result<()> {
    if !valid_conversation(id) {
        return Err(std::io::Error::other(format!(
            "agy reported a conversation id {id:?} that is not letters, digits and '-'"
        )));
    }
    let root = home.join(".gemini/antigravity-cli");
    let conversations = root.join("conversations");
    let results = [
        absent_ok(std::fs::remove_file(conversations.join(format!("{id}.db")))),
        absent_ok(std::fs::remove_file(
            conversations.join(format!("{id}.db-shm")),
        )),
        absent_ok(std::fs::remove_file(
            conversations.join(format!("{id}.db-wal")),
        )),
        absent_ok(std::fs::remove_dir_all(root.join("brain").join(id))),
    ];
    results.into_iter().collect()
}

fn absent_ok(result: std::io::Result<()>) -> std::io::Result<()> {
    match result {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> PathBuf {
        std::env::temp_dir().join(format!("miyu-antigravity-ws-{}", unique()))
    }

    #[test]
    fn unique_names_differ() {
        assert_ne!(unique(), unique());
    }

    #[test]
    fn work_dirs_are_new_and_removable() {
        let state = scratch();
        let a = create_work_dir(&state).unwrap();
        let b = create_work_dir(&state).unwrap();
        assert_ne!(a, b);
        assert!(a.starts_with(state.join("work")));
        assert_eq!(std::fs::read_dir(&a).unwrap().count(), 0);
        std::fs::write(a.join("x"), "1").unwrap();
        remove_work_dir(&a).unwrap();
        assert!(!a.exists());
        remove_work_dir(&a).unwrap();
        std::fs::remove_dir_all(&state).unwrap();
    }

    #[test]
    fn conversation_files_go_and_others_stay() {
        let home = scratch();
        let root = home.join(".gemini/antigravity-cli");
        std::fs::create_dir_all(root.join("conversations")).unwrap();
        std::fs::create_dir_all(root.join("brain/abc-1/.system_generated")).unwrap();
        std::fs::create_dir_all(root.join("brain/other")).unwrap();
        for name in ["abc-1.db", "abc-1.db-shm", "abc-1.db-wal", "other.db"] {
            std::fs::write(root.join("conversations").join(name), "x").unwrap();
        }
        remove_conversation(&home, "abc-1").unwrap();
        let left: Vec<String> = std::fs::read_dir(root.join("conversations"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(left, ["other.db"]);
        assert!(!root.join("brain/abc-1").exists());
        assert!(root.join("brain/other").exists());
        remove_conversation(&home, "abc-1").unwrap();
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn odd_conversation_ids_are_refused() {
        for id in ["", "..", "a/b", "a.b", "x y", "../../etc"] {
            assert!(!valid_conversation(id), "{id:?}");
            assert!(remove_conversation(Path::new("/nonexistent"), id).is_err());
        }
        assert!(valid_conversation("ff223be4-3376-4486-b661-7414044371c2"));
    }
}

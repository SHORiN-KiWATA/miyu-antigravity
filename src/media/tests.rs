use serde_json::json;

use super::*;
use crate::workspace;

fn scratch() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("miyu-antigravity-media-{}", workspace::unique()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn hash(digit: char) -> String {
    format!("sha256:{}", digit.to_string().repeat(64))
}

/// 一份存一个文件，名字是哈希加扩展名，内容是解开的字节；再存一次路径不变、只摸新。
#[test]
fn blobs_are_saved_by_hash_with_their_extension() {
    let dir = scratch();
    let params = json!({"blobs": {
        hash('a'): {"media_type": "image/png", "data": "iVBORw=="},
        hash('b'): {"media_type": "application/pdf", "data": "JVBERi0="},
    }});
    let saved = save(&params, &dir);
    let png = &saved[&hash('a')];
    assert_eq!(png, &dir.join(format!("{}.png", "a".repeat(64))));
    assert_eq!(std::fs::read(png).unwrap(), b"\x89PNG");
    assert_eq!(std::fs::read(&saved[&hash('b')]).unwrap(), b"%PDF-");
    assert_eq!(save(&params, &dir), saved, "same paths the second time");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// 写法不对的那一份不存、不在交回里，别的照存；没有 `blobs` 的什么都不存。
#[test]
fn a_bad_blob_is_skipped() {
    let dir = scratch();
    let params = json!({"blobs": {
        "md5:abc": {"media_type": "image/png", "data": "iVBORw=="},
        hash('c'): {"media_type": "image/png", "data": "not base64!"},
        hash('d'): {"media_type": "image/gif", "data": "R0lG"},
    }});
    let saved = save(&params, &dir);
    assert_eq!(saved.keys().collect::<Vec<_>>(), [&hash('d')]);
    assert!(save(&json!({}), &dir).is_empty());
    let left: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().collect();
    assert_eq!(left.len(), 1, "no partial files left behind");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// 一天没被摸过的删掉，新的留着。
#[test]
fn files_unused_for_a_day_are_swept() {
    let dir = scratch();
    let old = dir.join("old.png");
    let new = dir.join("new.png");
    std::fs::write(&old, b"x").unwrap();
    std::fs::write(&new, b"x").unwrap();
    let now = SystemTime::now();
    std::fs::File::options()
        .append(true)
        .open(&old)
        .unwrap()
        .set_modified(now - KEEP - Duration::from_mins(1))
        .unwrap();
    sweep(&dir, now);
    assert!(!old.exists());
    assert!(new.exists());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn unknown_types_get_bin() {
    assert_eq!(extension("image/jpeg"), "jpg");
    assert_eq!(extension("application/x-whatever"), "bin");
}

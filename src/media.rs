//! 请求里的图、文件存成文件，agy 照路径用它自己的 `view_file` 看（施工 AG-9）：agy 的输入只收文字（流式输入别的块一律报
//! 「only "text"」），图、PDF 只能由它的模型调 `view_file` 读。核心交的 `blobs`（清单声明了 `images`、`files`）一份存一个文件，
//! 放在 `<MIYU_STATE_DIR>/media/<哈希>.<扩展名>`：按内容起名，同一份每次都是同一个路径，转写出来的字逐字节不变。
//!
//! 回收：每次存的时候，这一次要用的摸新修改时间；一天没被摸过的删掉。还在对话里的图、文件每次请求都会交过来、都会被摸新。

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use base64::Engine;
use serde_json::Value;

/// 多久没用过的文件删掉。
pub const KEEP: Duration = Duration::from_hours(24);

/// 把 `params.blobs` 存进 `dir`，交回「blob 哈希 → 文件路径」；存不成的那一份不在里面（转写照旧写看不到）、记一行日志。
#[must_use]
pub fn save(params: &Value, dir: &Path) -> BTreeMap<String, PathBuf> {
    let Some(blobs) = params.get("blobs").and_then(Value::as_object) else {
        return BTreeMap::new();
    };
    if let Err(error) = std::fs::create_dir_all(dir) {
        eprintln!("media: can't create {}: {error}", dir.display());
        return BTreeMap::new();
    }
    let mut saved = BTreeMap::new();
    for (hash, blob) in blobs {
        match save_one(hash, blob, dir) {
            Ok(path) => {
                saved.insert(hash.clone(), path);
            }
            Err(why) => eprintln!("media: blob {hash} not saved: {why}"),
        }
    }
    sweep(dir, SystemTime::now());
    saved
}

/// 存一份：已经有了的只摸新修改时间，没有的解 base64、先写临时文件再改名。
fn save_one(hash: &str, blob: &Value, dir: &Path) -> Result<PathBuf, String> {
    let hex = hash
        .strip_prefix("sha256:")
        .filter(|hex| hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .ok_or("the hash is not sha256:<64 hex digits>")?;
    let media_type = blob
        .get("media_type")
        .and_then(Value::as_str)
        .ok_or("media_type is missing")?;
    let path = dir.join(format!("{hex}.{}", extension(media_type)));
    if path.is_file() {
        touch(&path).map_err(|error| error.to_string())?;
        return Ok(path);
    }
    let data = blob
        .get("data")
        .and_then(Value::as_str)
        .ok_or("data is missing")?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|error| format!("data is not base64: {error}"))?;
    let partial = dir.join(format!(".{hex}.partial"));
    let written = std::fs::File::create(&partial)
        .and_then(|mut file| file.write_all(&bytes).and_then(|()| file.sync_all()))
        .and_then(|()| std::fs::rename(&partial, &path));
    if let Err(error) = written {
        drop(std::fs::remove_file(&partial));
        return Err(error.to_string());
    }
    Ok(path)
}

/// 摸新修改时间。
fn touch(path: &Path) -> std::io::Result<()> {
    std::fs::File::options()
        .append(true)
        .open(path)?
        .set_modified(SystemTime::now())
}

/// 删掉 `dir` 里 [`KEEP`] 以上没被摸过的文件；删不掉的下次再删。
pub fn sweep(dir: &Path, now: SystemTime) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let stale = entry
            .metadata()
            .ok()
            .filter(std::fs::Metadata::is_file)
            .and_then(|meta| meta.modified().ok())
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age > KEEP);
        if stale {
            drop(std::fs::remove_file(entry.path()));
        }
    }
}

/// 照类型起的扩展名：`view_file` 照扩展名认图、PDF；不认识的用 `bin`。
#[must_use]
pub fn extension(media_type: &str) -> &'static str {
    match media_type {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/heic" => "heic",
        "image/bmp" => "bmp",
        "image/svg+xml" => "svg",
        "application/pdf" => "pdf",
        "audio/mpeg" => "mp3",
        "audio/wav" | "audio/x-wav" => "wav",
        "audio/ogg" => "ogg",
        "audio/aac" => "aac",
        "audio/flac" => "flac",
        "audio/mp4" => "m4a",
        "video/mp4" => "mp4",
        "video/webm" => "webm",
        "video/quicktime" => "mov",
        "text/plain" => "txt",
        "text/markdown" => "md",
        "text/csv" => "csv",
        "application/json" => "json",
        _ => "bin",
    }
}

#[cfg(test)]
mod tests;

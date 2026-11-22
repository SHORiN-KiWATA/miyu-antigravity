//! 系统提示词写成 agy 的自定义代理（设计第二节「系统提示词」、第九节）：`<家目录>/.gemini/config/agents/<名>/agent.md`，
//! 名字是 `miyu-` 加「白名单 + `request.system`」的 SHA-256 前 16 位十六进制（白名单变了名字跟着变，旧文件不会被当成新的用），
//! `--agent <名>` 选它。frontmatter 照旧版 Miyu 的 `antigravity/setup.rs`；`tools:` 是 agy 原生工具的白名单，只开 [`TOOLS`]
//! 里的（设计第七节）。正文是 `request.system` 原文。
//!
//! 回收（设计第八节「清理」）：每次写代理文件时，删掉 `miyu-` 开头、`agent.md` 一小时没改过、又不是活着的对话在用的代理
//! 目录（[`sweep`]）；活着的对话每接一轮把它的 `agent.md` 摸新（[`touch`]），别的 Miyu 进程回收时也不会删它。

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

use crate::words;

/// 代理文件里开着的 agy 原生工具（白名单）：`search_web`（设计第七节），和看图、读文件用的 `view_file`（施工 AG-9，项目主人
/// 2026-11-22 定：agy 的输入只收文字，图、文件只能由它自己读；`view_file` 读得到本机任何文件，不受 Miyu 的权限管，主人知道
/// 这一点）。清单 `hosted` 照这个写。
pub const TOOLS: &[&str] = &["search_web", "view_file"];

/// 代理的名字：`miyu-` 加「[`TOOLS`]（逗号连起来）、一个 NUL、`system`」的 SHA-256 前 16 位十六进制。
#[must_use]
pub fn name(system: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(TOOLS.join(",").as_bytes());
    hasher.update([0u8]);
    hasher.update(body(system).as_bytes());
    let digest = hasher.finalize();
    format!("miyu-{}", hex(&digest[..8]))
}

/// 字节写成小写十六进制。
#[must_use]
pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push(char::from(DIGITS[usize::from(byte >> 4)]));
        text.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    text
}

/// 代理文件的正文：`system` 原文，空一行，接 [`words::AGENT_NOTE`]。
#[must_use]
pub fn body(system: &str) -> String {
    format!("{system}\n\n{}", words::AGENT_NOTE)
}

/// 代理文件的全文。
#[must_use]
pub fn content(name: &str, system: &str) -> String {
    format!(
        "---\nname: {name}\n\
         description: Miyu system prompt (written by the Miyu antigravity package; renamed whenever the prompt changes)\n\
         mainAgent: true\nsubagent: false\ntools: [{}]\n---\n\n{}",
        TOOLS.join(", "),
        body(system)
    )
}

/// 代理目录多久没用过就回收。
pub const MAX_AGE: Duration = Duration::from_hours(1);

/// 代理目录的父目录。
#[must_use]
pub fn agents_dir(home: &Path) -> PathBuf {
    home.join(".gemini/config/agents")
}

/// 代理文件在 `home` 下的位置。
#[must_use]
pub fn path(home: &Path, name: &str) -> PathBuf {
    agents_dir(home).join(name).join("agent.md")
}

/// 把 `system` 写成 `home` 下的代理，交名字。先写进同目录的暂存文件再改名：几次调用同时写同一个代理时，agy 读到的总是整份。
///
/// # Errors
///
/// 建目录、写、改名出错：原样交。
pub fn write(home: &Path, system: &str) -> std::io::Result<String> {
    let name = name(system);
    let path = path(home, &name);
    let Some(dir) = path.parent() else {
        return Err(std::io::Error::other("the agent file has no parent folder"));
    };
    std::fs::create_dir_all(dir)?;
    let staged = dir.join(format!(".agent.md.{}", crate::workspace::unique()));
    std::fs::write(&staged, content(&name, system))?;
    if let Err(error) = std::fs::rename(&staged, &path) {
        let _ = std::fs::remove_file(&staged);
        return Err(error);
    }
    Ok(name)
}

/// 把代理 `name` 的 `agent.md` 的修改时刻改成现在。
///
/// # Errors
///
/// 打不开、改不了：原样交。
pub fn touch(home: &Path, name: &str) -> std::io::Result<()> {
    std::fs::File::options()
        .write(true)
        .open(path(home, name))?
        .set_modified(SystemTime::now())
}

/// 删掉 `home` 下名字以 `miyu-` 开头、`agent.md` 的修改时刻比 `now` 早 `max_age` 以上、又不在 `keep` 里的代理目录。别的名字的、
/// 没有 `agent.md` 的不动；代理的父目录还没有的不算错。
///
/// # Errors
///
/// 读目录、删目录出错：交第一个错，别的照样删。
pub fn sweep<S: std::hash::BuildHasher>(
    home: &Path,
    keep: &HashSet<String, S>,
    now: SystemTime,
    max_age: Duration,
) -> std::io::Result<()> {
    let entries = match std::fs::read_dir(agents_dir(home)) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let mut first_error = None;
    for entry in entries {
        let result = entry.and_then(|entry| {
            let name = entry.file_name();
            let Some(name) = name.to_str().filter(|name| name.starts_with("miyu-")) else {
                return Ok(());
            };
            if keep.contains(name) {
                return Ok(());
            }
            let modified = match std::fs::metadata(entry.path().join("agent.md")) {
                Ok(metadata) => metadata.modified()?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(error) => return Err(error),
            };
            if now.duration_since(modified).is_ok_and(|age| age > max_age) {
                std::fs::remove_dir_all(entry.path())?;
            }
            Ok(())
        });
        if let Err(error) = result {
            first_error.get_or_insert(error);
        }
    }
    first_error.map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_is_the_first_16_hex_digits_of_sha256() {
        // 「白名单 NUL 正文」的 SHA-256：正文是 system、空一行、`words::AGENT_NOTE`。
        assert_eq!(name("You are helpful."), "miyu-576e7390b131d299");
        // 白名单只有 search_web 那一版（0.1.3 以前）的名字不会再撞上。
        assert_ne!(name("You are helpful."), "miyu-e50ecc69ba43146b");
        // 正文里没有那段说明的旧名字不会再撞上。
        assert_ne!(name("You are helpful."), "miyu-c596dc5fc422a390");
        // 只哈希 system 的旧名字（白名单是空的那一版）不会再撞上。
        assert_ne!(name("You are helpful."), "miyu-42ea835592c20e1e");
        assert_eq!(name("").len(), "miyu-".len() + 16);
        assert_ne!(name("a"), name("b"));
    }

    #[test]
    fn content_has_the_frontmatter_and_the_prompt_verbatim() {
        assert_eq!(
            content("miyu-0011223344556677", "Line one.\n\nLine two."),
            format!(
                "---\nname: miyu-0011223344556677\n\
             description: Miyu system prompt (written by the Miyu antigravity package; renamed whenever the prompt changes)\n\
             mainAgent: true\nsubagent: false\ntools: [search_web, view_file]\n---\n\nLine one.\n\nLine two.\n\n{}",
                crate::words::AGENT_NOTE
            )
        );
    }

    #[test]
    fn the_whitelist_is_search_web_and_view_file_and_the_name_follows_it() {
        assert_eq!(TOOLS, ["search_web", "view_file"]);
        let text = content("n", "s");
        assert!(
            text.contains("\ntools: [search_web, view_file]\n"),
            "{text}"
        );
        assert!(!text.contains("tools: []"), "{text}");
        // 名字的输入是「白名单 NUL 正文」：自己算一遍对得上，白名单换一份、正文少了那段说明，名字就变。
        let digest = |input: &str| hex(&Sha256::digest(input.as_bytes())[..8]);
        let body = body("s");
        assert_eq!(
            name("s"),
            format!("miyu-{}", digest(&format!("search_web,view_file\0{body}")))
        );
        assert_ne!(name("s"), format!("miyu-{}", digest("search_web\0s")));
        assert_ne!(name("s"), format!("miyu-{}", digest(&body)));
        assert_ne!(name("s"), format!("miyu-{}", digest(&format!("\0{body}"))));
        assert_ne!(
            name("s"),
            format!(
                "miyu-{}",
                digest(&format!("search_web,read_url_content\0{body}"))
            )
        );
    }

    #[test]
    fn write_puts_it_under_the_given_home() {
        let home = std::env::temp_dir().join(format!(
            "miyu-antigravity-agent-{}",
            crate::workspace::unique()
        ));
        let name = write(&home, "Be brief.").unwrap();
        let file = home
            .join(".gemini/config/agents")
            .join(&name)
            .join("agent.md");
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            content(&name, "Be brief.")
        );
        // 再写一次照样成，目录里不留暂存文件。
        assert_eq!(write(&home, "Be brief.").unwrap(), name);
        let entries: Vec<_> = std::fs::read_dir(file.parent().unwrap()).unwrap().collect();
        assert_eq!(entries.len(), 1);
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn sweep_removes_only_old_unused_miyu_agents() {
        let home = std::env::temp_dir().join(format!(
            "miyu-antigravity-sweep-{}",
            crate::workspace::unique()
        ));
        sweep(&home, &HashSet::new(), SystemTime::now(), MAX_AGE).unwrap();
        let old = SystemTime::now() - Duration::from_hours(2);
        let agents = home.join(".gemini/config/agents");
        for name in ["miyu-old", "miyu-in-use", "miyu-new", "someone-else"] {
            write_named(&home, name);
        }
        for name in ["miyu-old", "miyu-in-use", "someone-else"] {
            set_modified(&home, name, old);
        }
        std::fs::create_dir_all(agents.join("miyu-no-file")).unwrap();
        let keep = HashSet::from(["miyu-in-use".to_string()]);
        sweep(&home, &keep, SystemTime::now(), MAX_AGE).unwrap();
        let mut left: Vec<String> = std::fs::read_dir(&agents)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(
            left,
            ["miyu-in-use", "miyu-new", "miyu-no-file", "someone-else"]
        );

        // 摸新以后不再算旧。
        touch(&home, "miyu-in-use").unwrap();
        sweep(&home, &HashSet::new(), SystemTime::now(), MAX_AGE).unwrap();
        assert!(agents.join("miyu-in-use").exists());
        std::fs::remove_dir_all(&home).unwrap();
    }

    fn write_named(home: &Path, name: &str) {
        let file = path(home, name);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "x").unwrap();
    }

    fn set_modified(home: &Path, name: &str, at: SystemTime) {
        std::fs::File::options()
            .write(true)
            .open(path(home, name))
            .unwrap()
            .set_modified(at)
            .unwrap();
    }
}

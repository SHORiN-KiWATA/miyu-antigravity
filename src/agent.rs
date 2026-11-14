//! 系统提示词写成 agy 的自定义代理（设计第二节「系统提示词」、第九节）：`<家目录>/.gemini/config/agents/<名>/agent.md`，
//! 名字是 `miyu-` 加 `request.system` 的 SHA-256 前 16 位十六进制，`--agent <名>` 选它。frontmatter 照旧版 Miyu 的
//! `antigravity/setup.rs`；`tools: []` 把 agy 的原生工具全关（AG-1 没有工具）。正文是 `request.system` 原文。

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// 代理的名字：`miyu-` 加 `system` 的 SHA-256 前 16 位十六进制。
#[must_use]
pub fn name(system: &str) -> String {
    let digest = Sha256::digest(system.as_bytes());
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

/// 代理文件的全文。
#[must_use]
pub fn content(name: &str, system: &str) -> String {
    format!(
        "---\nname: {name}\n\
         description: Miyu system prompt (written by the Miyu antigravity package; renamed whenever the prompt changes)\n\
         mainAgent: true\nsubagent: false\ntools: []\n---\n\n{system}"
    )
}

/// 代理文件在 `home` 下的位置。
#[must_use]
pub fn path(home: &Path, name: &str) -> PathBuf {
    home.join(".gemini/config/agents")
        .join(name)
        .join("agent.md")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_is_the_first_16_hex_digits_of_sha256() {
        // printf 'You are helpful.' | sha256sum
        assert_eq!(name("You are helpful."), "miyu-42ea835592c20e1e");
        assert_eq!(name("").len(), "miyu-".len() + 16);
        assert_ne!(name("a"), name("b"));
    }

    #[test]
    fn content_has_the_frontmatter_and_the_prompt_verbatim() {
        assert_eq!(
            content("miyu-0011223344556677", "Line one.\n\nLine two."),
            "---\nname: miyu-0011223344556677\n\
             description: Miyu system prompt (written by the Miyu antigravity package; renamed whenever the prompt changes)\n\
             mainAgent: true\nsubagent: false\ntools: []\n---\n\nLine one.\n\nLine two."
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
}

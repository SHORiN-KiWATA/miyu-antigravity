//! `unregister`：把本包写进 agy 配置的东西撤掉（设计第十一节第 2 条），给卸载前用。照进程自己的 `HOME`：
//!
//! - `<HOME>/.gemini/config/mcp_config.json` 里 `mcpServers.miyu` 一条拿掉，别的不动；文件读不懂的不写；文件没有、条目本来
//!   就没有算成功（[`mcp_config::unregister`]）。
//! - `<HOME>/.gemini/config/agents/` 下名字以 `miyu-` 开头的目录删掉（只删这个前缀的目录，不跟符号链接，别的名字的不碰）。
//!
//! 两件事互不牵连：一件出错另一件照做，退出码 1，错说在标准错误上。

use std::path::Path;
use std::process::ExitCode;

use crate::{agent, mcp_config, workspace};

/// 跑 `unregister`：成功退出码 0、标准输出写做了什么；出错退出码 1、原因写在标准错误。
#[must_use]
pub fn run() -> ExitCode {
    let Some(home) = workspace::home() else {
        eprintln!("HOME is not set, so there is no agy configuration to clean");
        return ExitCode::from(1);
    };
    let mut failed = false;
    match mcp_config::unregister(&mcp_config::path(&home)) {
        Ok(true) => println!(
            "removed the \"miyu\" entry from {}",
            mcp_config::path(&home).display()
        ),
        Ok(false) => println!(
            "no \"miyu\" entry in {}; nothing to remove",
            mcp_config::path(&home).display()
        ),
        Err(error) => {
            eprintln!("{error}");
            failed = true;
        }
    }
    match remove_agents(&home) {
        Ok(count) => println!(
            "removed {count} miyu-* agent director{} under {}",
            if count == 1 { "y" } else { "ies" },
            agent::agents_dir(&home).display()
        ),
        Err(error) => {
            eprintln!("cleaning the agent directories failed: {error}");
            failed = true;
        }
    }
    if failed {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

/// 删掉 `home` 下 `miyu-` 开头的代理目录，交删了几个；目录不存在算 0 个。符号链接、普通文件不删。
///
/// # Errors
///
/// 读不了目录、删不掉：交第一个错，别的照样删。
pub fn remove_agents(home: &Path) -> std::io::Result<usize> {
    let entries = match std::fs::read_dir(agent::agents_dir(home)) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    };
    let mut removed = 0;
    let mut first_error = None;
    for entry in entries {
        let result = entry.and_then(|entry| {
            let wanted = entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with("miyu-"));
            if wanted && entry.file_type()?.is_dir() {
                std::fs::remove_dir_all(entry.path())?;
                return Ok(1);
            }
            Ok(0)
        });
        match result {
            Ok(count) => removed += count,
            Err(error) => {
                first_error.get_or_insert(error);
            }
        }
    }
    first_error.map_or(Ok(removed), Err)
}

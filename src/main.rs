//! miyu-antigravity：Miyu 的扩展包 `antigravity`，把本机的 Antigravity CLI（`agy`）当模型供应商（`docs/00-设计.md`）。
//!
//! - `miyu-antigravity serve`：核心拉起的扩展，提供模型驱动 `antigravity/antigravity`。

use std::process::ExitCode;

const USAGE: &str = "usage: miyu-antigravity serve   (started by the Miyu core; provides the model driver antigravity/antigravity)";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("-h" | "--help" | "help") => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

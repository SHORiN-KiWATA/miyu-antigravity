//! miyu-antigravity：Miyu 的扩展包 `antigravity`，把本机的 Antigravity CLI（`agy`）当模型供应商（`docs/00-设计.md`）。
//!
//! - `miyu-antigravity serve`：核心拉起的扩展，提供模型驱动 `antigravity/antigravity`（[`rpc`]）。一次 `driver.call`（[`call`]）
//!   认得出是哪段活着的 agy 对话（[`sessions`]、[`registry`]）的下一轮的，只把新的话写进那个 agy（[`live`]）；认不上的新开
//!   一段，整份历史转写成一条文字（[`transcript`]）。agy 的流（[`stream`]）照 [`turn`] 交成增量。
//!
//! 这一版是施工 AG-2：还没有工具，agy 的原生工具全关。

mod agent;
mod agy;
mod call;
mod config;
mod failure;
mod live;
mod models;
mod process;
mod registry;
mod rpc;
mod sessions;
mod stream;
mod transcript;
mod turn;
mod words;
mod workspace;

use std::process::ExitCode;

const USAGE: &str = "usage: miyu-antigravity serve   (started by the Miyu core; provides the model driver antigravity/antigravity)";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("serve") => rpc::serve(std::io::stdin().lock(), &rpc::Out::stdout()),
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

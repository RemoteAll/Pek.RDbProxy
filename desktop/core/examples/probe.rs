//! 命令行自检（无 GUI）：服务器/CI 上快速判断代理是否可用。
//!
//! ```text
//! cargo run --example probe -- "Server=http://127.0.0.1:5599;Database=demo;Password=<令牌>;provider=network"
//! ```
//!
//! 退出码：`0` 全部通过、`1` 有失败项、`2` 参数错误。

use pek_rdbproxy_tool_core::{check, CheckStatus, SelfCheckOptions, Target};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args[0] == "-h" || args[0] == "--help" {
        eprintln!(
            "用法：probe \"Server=http://127.0.0.1:5599;Database=<数据源名>;Password=<令牌>;provider=network\"\n\
             可选：--sql \"SELECT 1\" --table <表名> --write \"UPDATE …\" --wrong-token"
        );
        std::process::exit(2);
    }

    let target = match Target::parse(&args[0]) {
        Ok(target) => target,
        Err(e) => {
            eprintln!("连接串无效：{e}");
            std::process::exit(2);
        }
    };

    let mut options = SelfCheckOptions::default();
    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--sql" if index + 1 < args.len() => {
                options.sql = args[index + 1].clone();
                index += 2;
            }
            "--table" if index + 1 < args.len() => {
                options.table = args[index + 1].clone();
                index += 2;
            }
            "--write" if index + 1 < args.len() => {
                options.write_sql = args[index + 1].clone();
                index += 2;
            }
            "--wrong-token" => {
                options.check_wrong_token = true;
                index += 1;
            }
            other => {
                eprintln!("未知参数：{other}");
                std::process::exit(2);
            }
        }
    }

    let report = check::selfcheck(&target, &options);
    println!("目标：{}", report.target);
    for item in &report.items {
        let mark = match item.status {
            CheckStatus::Pass => "✔",
            CheckStatus::Fail => "✘",
            CheckStatus::Skip => "○",
        };
        println!(
            "{mark} {:<32} {:<8} {}",
            item.name,
            format!("{} ms", item.elapsed_ms),
            item.detail
        );
    }
    println!(
        "通过 {} / 失败 {} / 跳过 {}（总 {} ms）",
        report.passed, report.failed, report.skipped, report.total_ms
    );
    if !report.ok() {
        std::process::exit(1);
    }
}

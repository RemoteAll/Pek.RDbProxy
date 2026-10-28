//! 命令行验证：登录代理 + 检查工具更新（与桌面工具内「登录代理」「检测更新」走同一条代码）。
//!
//! ```text
//! cargo run -p pek-rdbproxy-tool-core --example proxy_probe -- <代理地址> <用户名> <密码>
//! ```
//!
//! 输出：登录身份与权限、代理上托管的工具版本、是否需要升级。

use pek_rdbproxy_tool_core as tool;

fn main() {
    let mut args = std::env::args().skip(1);
    let server = args.next().unwrap_or_else(|| "http://127.0.0.1:5599".into());
    let user = args.next().unwrap_or_else(|| "admin".into());
    let password = args.next().unwrap_or_else(|| "admin".into());

    println!("== 当前工具版本：v{}", tool::CURRENT_VERSION);

    println!("\n== 登录 {server}（用户 {user}）");
    match tool::proxy_login(&server, &user, &password) {
        Ok(session) => {
            println!("  用户：{}（builtin={}）", session.user, session.builtin);
            println!("  权限：{}", session.permission_summary());
            if let Some(new_password) = session.new_password.as_deref() {
                println!("  ⚠ 面板把默认弱口令改成了随机密码：{new_password}（请立刻记下）");
            }
            println!("  能读 demo：{}　能写 demo：{}", session.can_read("demo"), session.can_write("demo"));
        }
        Err(e) => println!("  登录失败：{e}"),
    }

    println!("\n== 检查更新");
    match tool::update::check(&server) {
        Ok(info) => {
            println!("  代理上：{}", if info.present { format!("v{}", info.latest) } else { "（未上传工具包）".into() });
            println!("  大小：{} 字节　SHA-256：{}", info.size, info.sha256);
            println!("  上传时间：{}　说明：{}", info.uploaded_at, info.notes);
            println!("  是否需要升级：{}", info.newer);
        }
        Err(e) => println!("  检查失败：{e}"),
    }
}

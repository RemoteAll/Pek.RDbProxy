# Pek.RDbProxy 桌面工具（Tauri 2）

给代理配套的**桌面客户端**：填一行项目侧连接串，就能验证代理是否可用，也能当日常查数据的小工具。

- **连接测试**：`Login` → 显示 `DbType` / `Version`（同时验证令牌与数据源名）
- **SQL 控制台**：查询结果表格、`Execute` 受影响行数、`GetTables`、`QueryCount`
- **协议自检**：一键跑「Login → GetTables → Query →（可选）QueryCount / Execute → 错误信封」，
  每项给 ✔/✘/○ 与耗时，失败给可操作原因

> 它本身就是**一个真实消费方**：请求走 `Pek.RCode` 的 `DbClient`，与项目里
> `Server=…;Database=<数据源名>;Password=<令牌>;provider=network` **完全同一条代码路径**，
> 所以这里跑通即说明真实客户端可用。

## 结构（照 `DHFangSite/TauriFangSite` 的成熟布局）

```
desktop/
├─ Cargo.toml          # 独立嵌套工作区（根 Cargo.toml 用 exclude 排除）
├─ core/               # 纯 Rust 逻辑：连接串解析 / 操作 / 自检（可 cargo test，进 CI）
│  ├─ src/{lib,client,check}.rs
│  ├─ tests/protocol_stub.rs   # 最小 HTTP 桩：断言协议路径与出站 UA
│  └─ examples/probe.rs        # 命令行自检（无 GUI，服务器/CI 可用）
├─ src-tauri/          # Tauri 外壳：只做命令绑定（async + spawn_blocking）
└─ ui/                 # 纯静态前端（frontendDist，无需 Vite/Node 构建链）
```

## 构建 / 运行

```powershell
# 只跑逻辑测试（秒级；不需要 Tauri 工具链）
cd desktop
cargo test -p pek-rdbproxy-tool-core

# 命令行自检（不需要 GUI）
cargo run --example probe -- "Server=http://127.0.0.1:5599;Database=demo;Password=<令牌>;provider=network"

# 开发运行桌面程序（需 WebView2，Win10/11 自带）
cargo run -p pek-rdbproxy-tool

# 打包安装包（需要 Tauri CLI：cargo install tauri-cli --version "^2"）
cargo tauri build
```

> 本机 `G:` 盘空间紧张时，可把编译产物挪走：
> `$env:CARGO_TARGET_DIR=<别处目录>`（**只是临时应急**，不要写进仓库配置）。

## 约定 / 注意

- **出站统一带 `User-Agent: pek-rdbproxy-tool/<版本>`**：服务端 WAF 打开 `block_empty_ua`
  时机器客户端不会被误拦（`tests/protocol_stub.rs` 有断言守住）。
- **连接串只存本机**（localStorage），不上传；「关于」页可一键清除。
- 业务逻辑一律放 `core/`，`src-tauri` 不加逻辑——这样不用起 GUI 就能回归。
- `DbClient` 是同步阻塞实现，命令一律 `async fn` + `spawn_blocking`，否则卡界面。
- 图标由 `tools/gen-icons.ps1` 生成（代码画，不引外部素材）；**该 .ps1 必须 UTF-8 with BOM**。

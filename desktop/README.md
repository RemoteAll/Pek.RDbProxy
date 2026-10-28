# Pek.RDbProxy 桌面工具（Tauri 2）

给代理配套的**桌面客户端**：左边按项目存连接与 SQL 片段，右边连上就能跑 SQL / 自检。

- **连接（按项目分组）**：名称 / 项目 / 代理地址 / 数据源名 / 令牌 / 备注；单击选中、双击直接连；
  列表带健康点（上次连接成功/失败），悬停可改 / 删
- **SQL 片段**：命名 + 备注 + 可绑定连接；单击填入、双击直接跑；悬停可改 / 删；按使用次数排序
- **执行历史**：用保存的连接执行后自动记录（SQL / 结果 / 行数 / 耗时），可一键「存为片段」、删除单条、清空
- **控制台**：查询 / 执行写语句 / 表结构 / 记录数；结果可复制 CSV / JSON；`Ctrl+Enter` 查询、`Ctrl+Shift+Enter` 执行写语句
- **协议自检**：Login → GetTables → Query →（可选）QueryCount / Execute → 错误信封，报告可一键复制
- **导入 / 导出**：JSON；导出含明文令牌（有醒目提示），导入按「名称 + 数据源」合并
- **草稿**：每个连接各存一份 SQL 草稿，切换连接不丢
- **登录代理**（顶栏「登录代理…」）：用面板账号登录后每个请求带用户令牌 —— 代理端能看出**是谁在查**、
  按这个人的**数据源权限**放行；令牌只留在进程内存，不落盘、不下发页面，且只在服务器地址一致时才附带
- **自升级**（关于页「检测更新」）：代理面板「设置 → 🖥 桌面工具」上传新版本后，这里比对版本 →
  下载 → 校大小/SHA-256/PE 头 → 原地替换并重启（走 `dhrust::upgrade::replace_file`）

> 它本身就是**一个真实消费方**：请求走 `Pek.RCode` 的 `DbClient`，与项目里
> `Server=…;Database=<数据源名>;Password=<令牌>;provider=network` **完全同一条代码路径**，
> 所以这里跑通即说明真实客户端可用。

## 数据与安全

```text
%APPDATA%\com.peikesmart.rdbproxytool\workspace.json   连接 + 片段 + 历史
%APPDATA%\com.peikesmart.rdbproxytool\.key             本机密钥（不导出、不入库）
```

- 令牌用 **AES-256-GCM** 加密后落盘，界面只显示掩码；执行时由 Rust 侧解密，**令牌不下发到前端**。
- 换电脑：导出 JSON（含明文令牌，注意保管）→ 新机器导入即可。

## 结构（照 `DHFangSite/TauriFangSite` 的成熟布局）

```
desktop/
├─ Cargo.toml          # 独立嵌套工作区（根 Cargo.toml 用 exclude 排除）
├─ core/               # 纯 Rust 逻辑：连接串解析 / 操作 / 自检 / 登录 / 升级 / 工作区存储（可 cargo test，进 CI）
│  ├─ src/{lib,client,check,auth,update,store}.rs
│  ├─ tests/protocol_stub.rs   # 最小 HTTP 桩：断言协议路径与出站 UA
│  └─ examples/{probe,ws_seed,proxy_probe}.rs   # 命令行自检 / 示例工作区 / 登录+检测更新
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

# 登录代理 + 检查工具更新（不需要 GUI；走与界面同一条代码）
cargo run --example proxy_probe -- http://127.0.0.1:5599 <用户名> <密码>

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

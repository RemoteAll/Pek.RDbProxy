# Pek.RDbProxy

> 数据库**调试代理**：把真实数据库凭据收敛在一台机器，让**没有数据库账号的项目**安全地调试数据层。

- crate：`pek-rdbproxy`（库 + 单文件程序），`v0.1.0`
- 仓库：`github.com/RemoteAll/Pek.RDbProxy`

---

## 它解决什么问题

开发/联调时经常需要把数据库账号发给"不该有账号的人"或写进别人的配置文件（随后进 git）。Pek.RDbProxy 让**凭据只留在这台机器**：

```
项目（本机/内网）                    代理机（有数据库账号）              数据库
DbClient ──http──► Pek.RDbProxy :5599 ──驱动──► MySQL / PG / SQLServer / SQLite …
   连接串：Server=http://10.0.0.5:5599;Database=dev;Password=<Token>;provider=network
```

- 项目侧只拿到 **一个令牌**，没有账号、没有密码、没有内网数据库地址。
- 每个数据源可单独设**只读**：写操作在服务端被拦下，注入式拼接也绕不过 `is_read_only_sql`。
- 全部请求进 **SQL 审计**（谁、什么时候、什么语句、耗时、结果）。

## 复用（不重复造轮子）

| 能力 | 来源 |
|---|---|
| HTTP 路由 / 中间件 | `dhrust::net::router` |
| 面板鉴权（None / LocalOnly / Full + Bearer） | `dhrust::net::panel_auth` |
| 用户令牌 JWT（HS256，与 C# NewLife 互通） | `pek-rpermissions` |
| WAF（SQLi / XSS / 爬虫 / CC） | `pek-rwaf` |
| 服务化（Windows 服务 / systemd / launchd） | `dhrust::service` |
| 星尘托管注册 | `dhrust::staragent` |
| 配置读写（保留注释） | `dhrust::config::toml` |
| 数据库访问与连接串解析 | `pek-rcode` |

## 快速开始

```powershell
$env:PEK_RDBPROXY_BASE='G:\Code\Rust\Pek.RDbProxy'
.\target\release\pek-rdbproxy.exe -init    # 生成 Config/Proxy.toml
.\target\release\pek-rdbproxy.exe          # 前台启动，面板 http://127.0.0.1:5599/
```

首次启动自动创建面板账号 **admin/admin**（面板会常显横幅提醒改密）。

### 面板

面板资源与 Pek.RAgent 完全同构（`res/index.html` 骨架 + `res/app.css` + `res/app.js`，由 `/assets/*` 提供，支持 ETag/304），仅内容区按本工具业务渲染；默认浅色主题，右上角 🌓 可切深色。

| 页签 | 内容 |
|---|---|
| 概览 | 统计卡（数据源/只读/请求/失败/平均耗时）+ 数据源卡片（状态/令牌/模式/请求量/连接串，含编辑·只读切换·删除）+ 项目侧连接串提示 |
| SQL 审计 | 时间 / 数据源 / 操作 / 结果 / 耗时 / 行数 / 令牌指纹 / SQL 摘要（内存保留 500 条） |
| 设置 | 监听端口 / 允许远程访问 / 鉴权级别 / 面板用户名·密码 |

### 项目侧接入

```rust
// 项目里无需任何数据库账号
let conn = "Server=http://127.0.0.1:5599;Database=demo;Password=<Token>;provider=network";
let dal = Pek.RCode::Dal::open(conn, model)?;
```

令牌在面板「新增数据源」时自动生成，或手工填写；连接串支持 `enc:v1:` 加密值（`-encrypt <明文>`）。

## 配置 `Config/Proxy.toml`

```toml
[Server]
Host = "127.0.0.1"
Port = 5599
AllowRemote = false
WebUserName = "admin"
WebPasswordHash = ""          # 空 = 首启用 admin/admin
WebAuthLevel = "LocalOnly"    # None / LocalOnly / Full
JwtSecret = ""                # 空 = 首启自动生成 HS256:<hex>

[[Databases]]
Name = "demo"
Connection = "Data Source=Data/demo.db;Provider=SQLite"
Token = "…"
ReadOnly = false
Description = "本地演示"
```

- 文件由程序写回时**保留注释与排版**；新增字段自动补入。
- 真实连接串只保存在本机，`.gitignore` 已排除 `Config/`。

## 命令行

| 参数 | 说明 |
|---|---|
| `-init` | 生成配置模板 |
| `-status` | 打印状态与数据源概览（密码掩码） |
| `-token` | 生成一个随机令牌（粘到新数据源的「令牌」里） |
| `-encrypt <plain>` | 生成 `enc:v1:` 加密串 |
| `-install` / `-uninstall` | 安装 / 卸载系统服务 |
| `-start` / `-stop` / `-restart` | 服务控制 |
| `-RegisterAgent` | 注册到星尘（影子模式：升级 = 直接覆盖程序文件即自动重启） |
| `-run` | 前台运行（默认） |

### 运行中升级

把新程序丢进 `Update/` 目录即可：静置 10 秒后自动校验 → 原子替换 → 退出并由服务/星尘拉起新版本。星尘托管下也可以直接覆盖源程序文件。

## 协议

`/Db/*` 与 Pek.RCode `provider=network` 的 `DbClient` 完全兼容：

| 方法 | 路径 | 说明 |
|---|---|---|
| GET/POST | `/Db/GetTables` | 表列表 |
| POST | `/Db/Query` | 查询 → `{"data": [{…}]}` |
| POST | `/Db/Execute` | 执行 → `{"data": 行数}` |
| POST | `/Db/GetSchema` | 表结构 |
| GET | `/Db/Version` | 版本信息 |

成功返回**裸**结果（无 `ok` 信封），失败返回 `{"code": <int>, "msg": "…"}`。

## 部署

打好的包在 `dist/`（不入库）：`pek-rdbproxy-v<版本>-win-x64.zip` / `-linux-x64.tar.gz` / `-linux-arm64.tar.gz` + `SHA256SUMS`。

```powershell
# Windows（解压后同目录执行）
.\install.ps1                          # 装到 C:\PekRDbProxy 并注册为 Windows 服务
.\install.ps1 -RegisterAgent           # 同时注册到星尘（影子模式）
.\install.ps1 -Target D:\Apps\RDbProxy # 自定义目录
```

```bash
# Linux（解压后执行）
sudo ./install.sh ./pek-rdbproxy /opt/PekRDbProxy
# 检测到 StarAgent 会自动注册托管；否则稍后手动：
/opt/PekRDbProxy/pek-rdbproxy -RegisterAgent
```

打包：

```powershell
.\scripts\pack.ps1              # 版本 +1，Windows + Linux x64/arm64（zig 交叉 musl 静态）
.\scripts\pack.ps1 -Targets linux
.\scripts\pack.ps1 -NoBump -SkipBuild   # 只重新打包已有产物
```

### 在线升级

三种方式任选，都会在 **10 秒内**自动完成替换并重启：

1. **面板** → 设置 → 「⬆ 在线升级」→ 选择新程序 → 上传并升级
2. 把程序拷进 `Update/` 目录（文件名随意）
3. 星尘托管下直接覆盖源程序文件（影子模式，运行中的文件不被占用）

升级过程：写入 `Update/` → 静置 10 秒 → PE/ELF 校验 → 原子替换（Windows 先改名让位）→ 退出并由服务/星尘拉起。失败会改名成 `*.failed` 并记日志。

## 安全

- **写盘加密**：`Config/Proxy.toml` 里的连接串一律以 `enc:v1:`（AES-256-GCM）保存，密钥在 `Config/.key`（Unix 权限 600）；启动时自动把仍是明文的连接串加密迁移。
- 默认只读；写操作需在面板显式解除。
- 所有请求带令牌；审计里令牌只留指纹（`Ve-g****j5aA`）。
- WAF 全站生效，`/Db` 与 `/api/upgrade` 豁免攻击特征（分别由只读护栏与文件头校验兜底）。
- `AllowRemote=false` 时非回环请求直接拒绝；面板鉴权级别可设 `Full`（全部需 JWT）。

## 开发

```powershell
cargo test --offline                    # 9 单测 + 2 端到端
cargo clippy --all-targets --offline    # 0 告警
```

接续与记忆：见 `docs/项目接续指南.md`、`docs/项目记忆.md`、`docs/决策记录.md`。

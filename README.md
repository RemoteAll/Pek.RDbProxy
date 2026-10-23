# Pek.RDbProxy

> **本地/内网数据库代理**：项目里只写「代理地址 + 令牌」，**真实连接串只留在代理端**——
> 代码、日志、截图、AI 上下文里都不会出现数据库凭据。

```text
# 项目里配置的就是这一行（不含任何真实凭据）
Server=http://127.0.0.1:5599;Database=demo;Password=tk-xxxx;provider=network
```

协议完全复用 [Pek.RCode](https://github.com/RemoteAll/Pek.RCode) 的 `/Db/*`（与 C# `DbController` 一致），
**Rust / C# 客户端都能直连**，本项目只负责「多数据源 + 令牌鉴权 + 只读护栏 + SQL 审计 + 面板」。

---

## 为什么需要它

| 直接连真实库 | 经 Pek.RDbProxy |
|---|---|
| 连接串（含账号密码）写在 `appsettings.json` / `.env` / 代码里 | 项目里只有 `127.0.0.1:5599` + 一个令牌 |
| 交给 AI 排查问题时，凭据随文件进入上下文 | AI 只看到代理地址与令牌，**拿不到真实凭据** |
| 调试期容易误写生产库 | `ReadOnly = true` 时写操作一律拒绝 |
| 不知道谁执行了什么 SQL | 每次请求都有审计（库 / 操作 / 耗时 / 行数 / SQL 摘要） |

---

## 快速开始

```powershell
# 1. 生成配置模板（Config/Proxy.toml，已在 .gitignore 里排除）
pek-rdbproxy -init

# 2. 生成一个项目侧令牌
pek-rdbproxy -token
# → LeMw7Ozjt9c7T_DXvjnBIA

# 3. 编辑 Config/Proxy.toml（填真实连接串 + 令牌）
# 4. 查看配置与连通性（连接串自动脱敏）
pek-rdbproxy -status

# 5. 启动
pek-rdbproxy
# → 面板 http://127.0.0.1:5599/
```

### 配置示例（`Config/Proxy.toml`）

```toml
[Server]
Host = "127.0.0.1"        # 只服务本机（推荐）；要跨机访问改 0.0.0.0 + AllowRemote = true
Port = 5599
AdminToken = ""           # 面板登录令牌；空 = 面板仅本机可访问
AllowRemote = false

[[Databases]]
Name = "demo"             # 项目侧 Database=demo
Connection = "Server=10.0.0.5;Port=3306;Database=real_db;Uid=readonly;Pwd=***;provider=mysql"
Token = "LeMw7Ozjt9c7T_DXvjnBIA"   # 项目侧 Password=<Token>
ReadOnly = true           # 调试期强烈建议 true
Description = "生产库（只读）"
```

支持的数据源：SQLite（内嵌）、MySQL/MariaDB、PostgreSQL/VastBase、SQL Server 等
（其余驱动在 `Cargo.toml` 的 `pek-rcode` 特性里按需添加 `driver-*`）。

---

## 项目侧怎么连

### Rust（`pek_rcode`）

```rust
let dal = Dal::open("Server=http://127.0.0.1:5599;Database=demo;Password=LeMw7Ozjt9c7T_DXvjnBIA;provider=network")?;
```

### C#（DH.NCode）

```csharp
var dal = DAL.Create("Server=http://127.0.0.1:5599;Database=demo;Password=LeMw7Ozjt9c7T_DXvjnBIA;provider=Network");
```

> 前提：C# 侧启用了 `Network` 驱动（与 C# `DbServer` 同一套协议）。

---

## Web 面板

- **概览**：每个数据源的类型 / 只读状态 / 可用性 / 请求数 / 失败数 / 平均耗时 / 脱敏连接串，
  可一键「设为只读 / 解除只读」（**运行期生效**，重启恢复配置文件的值）
- **SQL 审计**：最近 500 条请求（内存环形 + 写 `Log/`），含时间、库、操作、结果、耗时、行数、
  令牌指纹（`tk-d****e-me`）、SQL 摘要 / 错误信息

### 面板鉴权

| `AdminToken` | 行为 |
|---|---|
| 空（默认） | 面板仅**本机回环**可访问，无需登录 |
| 已设置 | 需要 `Authorization: Bearer <AdminToken>` |

`AllowRemote = false` 时，**非回环来源的请求一律 403**（含 `/Db/*`），防止误把代理暴露到公网。

---

## 只读护栏

`ReadOnly = true` 时：

- `/Db/Execute`、`/Db/InsertAndGetIdentity` → **直接 403**；
- `/Db/Query` 里若不是只读语句（`SELECT` / `SHOW` / `DESC` / `EXPLAIN` / `PRAGMA` / 只读 `WITH`）→ **403**。

> 只读是「尽力而为的护栏」：CTE 里夹带写关键字会被拒绝，但**建议同时在数据库侧给只读账号**（双重保险）。

---

## 连接串加密（可选）

```powershell
pek-rdbproxy -encrypt "Server=10.0.0.5;Database=real_db;Uid=readonly;Pwd=***;provider=mysql"
# → enc:v1:xxxxx...（填到 Proxy.toml 的 Connection）
```

密钥在 `Config/.key`（首次加密时生成），也可以用环境变量 `PEK_RDBPROXY_KEY` 覆盖。
**`Config/` 整个目录不入库**（`.gitignore` 已排除）。

> 说明：本机明文密钥只能防「配置文件被随手看到/被误提交」，不能防能读源码的攻击者；
> 真正的防线是「项目里根本没有凭据」。

---

## CLI

| 命令 | 说明 |
|---|---|
| `pek-rdbproxy` | 启动代理（读 `Config/Proxy.toml`） |
| `-init` | 生成配置模板（不覆盖已有文件） |
| `-status` | 配置概览 + 连通性（**连接串脱敏**） |
| `-token` | 生成随机令牌 |
| `-encrypt <连接串>` | 加密连接串（密钥存 `Config/.key`） |
| `-help` | 用法 |

环境变量：`PEK_RDBPROXY_BASE` 可指定程序基目录（默认 exe 所在目录）。

---

## 目录结构

```
Config/Proxy.toml   真实连接串 + 令牌（不入库）
Config/.key         加密密钥（可选，不入库）
Data/               数据源相对路径的落点（如 SQLite）
Log/                运行日志与 SQL 审计
```

---

## 开发

```powershell
cargo test        # 8 单测 + 2 端到端（真实 HTTP + DbClient）
cargo clippy --all-targets
```

端到端覆盖：登录、查询、只读拦截、令牌校验、数据源不存在、面板鉴权与审计。

> 首次构建若卡在 `rsproxy.cn` 下载，可复用同级仓库的 `Cargo.lock`（版本已锁定）后加 `--offline`。

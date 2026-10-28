# Pek.RDbProxy · 项目指令

> 全项目通用规则见用户级《跨项目接续协议》（本机 `%APPDATA%\Code\User\prompts\跨项目接续协议.instructions.md`；模板在 `tools/continuity/templates/`）。
> **开工先读（存在则读，按序）**：`.github/copilot-instructions.md` → `docs/项目接续指南.md` → `docs/用户档案.md` → `docs/项目记忆.md` → 决策/审计文档最新节。

## 项目定位

本地/内网**数据库代理**：项目侧只写「代理地址 + 令牌」，真实连接串只留在代理端
（协议复用 `Pek.RCode` 的 `/Db/*`，与 C# `DbController` 一致）。

## 依赖约定（不要重复造轮子）

| 能力 | 复用来源 | 说明 |
|---|---|---|
| 路由 / 中间件 | `dhrust::net::router`（`Router` + `Ctx`） | 不要手写 path 匹配 |
| 面板鉴权 | `dhrust::net::panel_auth`（`AuthLevel`/`bearer_token`/`client_ip`/`is_loopback`） | 与 Pek.RAgent 同款 |
| 登录限流 | `dhrust::net::login_guard` | 默认 15 分钟 5 次 |
| 用户令牌 | `pek-rpermissions::jwt`（HS256，与 C# NewLife `JwtBuilder` 互通） | 不要自己拼 JWT |
| 密码存储 | `dhrust::sign::salted_sha256_hex` + `dhrust::secret` | 不明文落盘 |
| WAF | `pek-rwaf`（`admin_api` 预设） | 所有带网页的项目必须接 |
| 数据库 | `pek-rcode`（`database` + `driver-*`） | — |
| 驱动按需下载 | `pek_rcode::driver_pack`（`driver-pack` 特性 + `[DriverStore]`） | 未内置的库类型运行时从组件源下载 dbserver 驱动包；**驱动升级不必重编本仓** |
| 服务化 | `dhrust::service::ServiceManager` | Windows SCM / systemd |
| 星尘托管 | `dhrust::staragent::register` | Mode=11 影子模式 |
| 自升级 | `Update/` 目录热检测 + 原子替换（参考 `Pek.RAgent`） | 覆盖即升级 |
| 日志 / 配置 / 加密 | `dhrust::logs` / `dhrust::config` / `dhrust::secret` | — |

## 每轮收尾（务必）

① 决策/审计文档新增一节（**含用户原话**）；② `tools/continuity/sync-memory.ps1` 同步记忆；③ 跑回归验证并如实汇报；④ 提交并推送。

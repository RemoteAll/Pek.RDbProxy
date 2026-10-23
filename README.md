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
| 字段安全 | 表管理：`allow`/`mask`/`deny` 列策略 + `enforce`（默认，未登记列不放行）/`audit` 模式 + 未登记字段一键登记（详见下文「字段级安全」） |
| 用户 | 多用户与页签权限（**仅内置管理员可见**）：新增/编辑/停用/删除，勾选可访问页签 |
| 设置 | 监听端口 / 允许远程访问 / 面板仅限本机 / 鉴权级别 / 在线升级 / 运行信息（内置管理员用户名·密码在「用户」页编辑） |

### 用户与权限

两类账号，登录时自动识别：

| 账号 | 来源 | 权限 |
|---|---|---|
| **内置管理员** | `Config/Proxy.toml` 的 `WebUserName` / `WebPasswordHash`（默认 `admin/admin`） | 全部页签 + 用户管理；不可删除 |
| **普通用户** | `Data/admin.db` 的 `RDbProxy_PanelUser`（「👥 用户」页维护） | 只给勾选的页签；越权请求返回 `403` |

- 权限按**页签/能力**授权：`overview`（数据源与概览，只读）/ `sources`（数据源增删改、取令牌、看真实连接串）/
  `fields`（字段安全）/ `audit`（SQL 审计）/ `settings`（服务设置与升级）。
- 关键区分：**只给 `overview` 的人只能看到脱敏连接串与统计**，拿不到令牌、改不了数据源（取令牌/真实连接串接口返回 `403`）。
- 无权限的页签在面板导航里**直接隐藏**；用 `#hash` 硬跳也会被弹回「概览」，后端同步返回 `403`（双保险）。
- **本机回环访问**（`WebAuthLevel = None / LocalOnly`）等同内置管理员 —— 能改配置文件的人本来就能管理一切。
- 数据层复用 `pek-radmin`（与 Pek.RAgent / HlkProductTool 同一套用户/权限组装线），存储 `Data/admin.db`，
  表结构见 `Entity/Model.xml`（XCode 规范，与 C# 生态互通）。
- 密码：**只要求非空**（不强制复杂度，与 `pek-radmin` 口径一致）；内置管理员的初始 `admin` 仍会在首次登录时
  自动换成随机强密码（只显示一次，可用 `-resetPassword` 重设）。

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
| `-reinstall` | 重装服务（卸载后按**当前程序路径**重新注册；服务已存在/注册项变旧时用） |
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

### 常见问题（FAQ）

**Q1 `install.ps1 : 无法将"install.ps1"项识别为 cmdlet、函数、脚本文件…`**

PowerShell 不从当前目录加载命令，必须加 `.\` 前缀：

```powershell
cd H:\Run\pek-rdbproxy
.\install.ps1
```

**Q1b `无法加载文件 …\install.ps1，因为在此系统上禁止运行脚本`（UnauthorizedAccess / PSSecurityException）**

这是 PowerShell 的**执行策略**（ExecutionPolicy）拦截，与脚本本身无关。四种解法，任选其一：

```powershell
# 方案 1（推荐，不改变系统设置）：只对本次执行放行
powershell -NoProfile -ExecutionPolicy Bypass -File .\install.ps1

# 方案 2：只对当前这个终端窗口放行，之后在本窗口内可直接 .\install.ps1
Set-ExecutionPolicy -Scope Process -ExecutionPolicy Bypass -Force

# 方案 3：对当前用户永久放行「本地脚本」（需管理员，安全但宽松）
Set-ExecutionPolicy -Scope CurrentUser -ExecutionPolicy RemoteSigned -Force

# 方案 4：组策略限制（企业环境常见，方案 1/2/3 都可能被覆盖）
#   查看生效来源：Get-ExecutionPolicy -List
#   若 MachinePolicy / UserPolicy 为 Restricted，需 IT 在组策略里放开，或改用
#   「用 cmd 直接跑 exe」的替代路径（见下）
```

替代路径（完全绕开 .ps1 执行策略）：

```powershell
cd H:\Run\pek-rdbproxy
.\pek-rdbproxy.exe -init          # 生成 Config/Proxy.toml
.\pek-rdbproxy.exe -install       # 注册并启动 Windows 服务
.\pek-rdbproxy.exe -RegisterAgent # 可选：注册星尘托管
```

即 `install.ps1` 做的事只是这三条命令 + 复制程序，手动执行等价。

**Q2 服务注册失败 / 防火墙规则没加上**

需要**管理员权限**：以管理员身份打开 PowerShell 再执行（注册 Windows 服务、调用 `netsh` 都需要）。

**Q3 面板打不开 / 远程访问不通**

1. 先看日志 `Log\yyyy_MM_dd.log`（`.\pek-rdbproxy.exe -status` 末行会打印日志目录与最近 3 行）
2. 确认在跑：`Get-Service PekRDbProxy` 或 `.\pek-rdbproxy.exe -status`
3. 本机访问 http://127.0.0.1:5599/
4. 远程访问：面板「设置」勾选「允许远程访问」—— **即时生效，无需重启**；程序会自动用 `netsh advfirewall`（Linux 为 firewalld/ufw/iptables）放行该 TCP 端口
5. 自动放行失败（多半是没管理员权限）时手动执行：
   `netsh advfirewall firewall add rule name="PekRDbProxy" dir=in action=allow protocol=TCP localport=5599`
6. 云服务器还需在**安全组**放行同一端口
7. 对外开放前把鉴权级别设为 `Full` 并先改掉默认密码

**Q4 忘记面板密码**

```powershell
.\pek-rdbproxy.exe -resetPassword "YourStrongPass1!"
.\pek-rdbproxy.exe -restart
```

规则：**只要求非空**（不强制复杂度，与 `pek-radmin` 口径一致）。建议自行使用不易猜的密码。

**Q5 登录后提示「默认密码已自动更换」**

首次用 `admin/admin` 登录时，程序会按强度规则生成强密码并回写配置，新密码**只在弹窗里显示一次**。没记下就用 Q4 重置。

**Q6 改端口 / 改远程访问需要重启吗？**

不用。保存后立即热重绑监听，新地址绑定失败会自动回滚到原监听，面板不会断。

**Q7 怎么升级**

设置页「⬆ 在线升级」上传新程序；或把程序放进 `Update/` 目录；星尘托管下也可直接覆盖源程序文件。约 10 秒自动替换并重启。

**Q8 安装提示「服务已存在：PekRDbProxy」/ 启动报 1053**

1053 是 Windows 服务启动超时，常见两个原因：**注册项指向旧路径/旧参数**（早期版本注册的 binPath 可能没有 `-s`，SCM 拉起后程序不认识参数立即退出），或**旧实例没退干净还占着 5599 端口**（新实例绑定失败→退出）。

```powershell
.\pek-rdbproxy.exe -reinstall   # 一步：卸载（停服务+删注册）→ 按当前程序重新注册并启动
# 或分开做：
.\pek-rdbproxy.exe -uninstall
.\pek-rdbproxy.exe -install
```

- `-uninstall` 会**等进程真正退出**再删注册；若报「服务未能停止」（进程不响应停止控制），先 `Get-Process pek-rdbproxy | Stop-Process -Force` 再试，或重启系统。
- 新版 `install.ps1` 已自动处理：检测到旧服务就直接 `-reinstall`，并把 `sc qc` 的注册信息打进日志。
- 确认注册项：`sc qc PekRDbProxy`，`BINARY_PATH_NAME` 应为 `"<程序路径>" -s`。

**Q9 服务停不下来 / 卸载后端口还被占用**

v0.1.7 起服务主体会响应 SCM 的 Stop（此前主循环无限等待、`sc stop` 只能超时，进程残留）。若在**旧版本**上遇到，手动 `Get-Process pek-rdbproxy | Stop-Process -Force`，再用新版 `-reinstall`。

## 字段级安全（表管理 / 列策略）

针对「有些字段无论如何都不能返回，但 ORM 默认 `SELECT *` 会请求所有列」的场景：

策略存放在 `Config/field-policy.json`（本机，不入库），按 **数据源 + 列名**（大小写不敏感）登记：

| 动作 | 含义 | `audit` 模式 | `enforce` 模式 |
|---|---|---|---|
| `allow` | 允许返回 | 返回 | 返回 |
| `mask` | **值脱敏**（列保留，每行值变 `***`） | 按 `allow` 处理并记录 | 列保留、值 `***` |
| `deny` | **永不返回** | **不返回**（硬拒绝，与模式无关） | **不返回** |
| 未登记（有备注但没设 action 也算未登记） | 按模式默认 | 返回 + 记审计「未登记字段」 | **不返回**（默认模式） |

要点：

- **`deny` 是硬拒绝**：即使 `audit`（学习）模式也**不会**返回该列 —— 这是给"绝对不能出去"的字段用的（密码、密钥、手机号等）。
- 请求方用 `SELECT *` 时，返回的结果集里**直接没有这一列**（不是空值）。ORM 按列名取会得到"字段不存在"，比返回空值安全——空值会和真实的 NULL 混淆。
- **`mask` 是"列在、值不在"**：列仍然返回、按列名能取到，但每行值都是 `***`（唯一值都变成同一个）——适合"结构要对得上、值不能看"的字段（如手机号、邮箱、身份证）。
- **默认模式是 `enforce`（严格白名单）：未登记列一律不返回**，包括"策略为空"的初始状态。
- **上线落地步骤**（避免"一装就所有列都没了"）：
  1. 先跑一次业务查询（或把项目连上来点一次）；
  2. 到「🛡 字段安全」的「未登记字段」点「一键允许全部」（后端 `POST /api/fields/bulk`，按审计观测到的列批量 `allow`）；
  3. 再把敏感列逐项改成 `deny` / `mask`。
- 想先观察一段时间就切 `audit`（只记审计、不裁剪），看完再切回 `enforce`。
- 每次拦截都会写入审计：「字段已拦截：a,b」「字段已脱敏：c,d」「未登记字段：e,f」，可在面板「SQL 审计」看到，并据此登记。
- **元数据接口（GetTables / GetSchema）不受约束**，只约束查询的数据行。
- 建议流程：先切 `audit` 跑一段时间 → 看审计里的「未登记字段」→ 登记 `allow` 白名单与 `deny`/`mask` → 再切回 `enforce`。

## 日志

程序目录下的 `Log/`，与 Pek 系其它服务同一套实现（`dhrust::logs`，格式对齐 DH.NCore）：

```
H:\Run\pek-rdbproxy\Log\2026_10_23.log
```

- 文件名 `yyyy_MM_dd.log`；单文件 10MB 封顶后拆 `_2`、`_3`…；目录内最多留 200 个文件（自动删最旧）。
- 行格式 `时间 线程ID 类型 名称 正文`，UTF-8 + CRLF，文件头 4 行字段说明（与 C# DH.NCore 的日志文件同款）。
- **只要执行过就一定有**：`-init` / `-status` / `-token` / `-install`… 都会落一条
  `Pek.RDbProxy v… 执行：args=[…] 程序目录=…`；进程退出前强制 flush，不会丢最后几条。
- 写什么：启动与监听地址、端口热重绑、防火墙放行、数据源增删改、面板登录成功/失败、
  每次 `/Db/*` 调用（`db=… op=… ok/FAIL … rows=… …us sql=…`）、字段策略拦截、自升级全过程。
- 排障：`.\pek-rdbproxy.exe -status` 末行会打印日志目录、当日文件名与**最近 3 行**。
- 程序目录不可写时（只读介质等）文件日志静默失败、不影响服务运行，`-status` 里没有「日志：」行即说明没写成。

## 部署模型（给开发调测，但不让他们看到不该看的数据）

本工具的定位是**本地调测**：真实连接串与数据库账号收敛在受控机器上，开发人员用自己的账号登录面板（或只拿令牌连 `/Db/*`），凭据不离开这台机器。

| 模型 | 适用 | 配置 |
|---|---|---|
| 本机自用 | 自己调自己 | 默认（`AllowRemote=false`） |
| **受控代理机（推荐）** | 一个负责人管数据源，多人从自己电脑登录面板调测 | `AllowRemote=true` + 每人一个账号（只授需要的页签）+ 需要时再开字段安全白名单 |
| 极端锁死 | 不想让其他人看到面板 | `PanelLocalOnly=true`（面板仅回环，远程只能令牌走 `/Db/*`） |

默认（`PanelLocalOnly=false`）：

- 开发人员**在任意电脑打开 `http://代理IP:5599/`，用自己的账号密码登录**（本机回环免登录）；
- 登录后**只看得到被授权的页签**，没权限的页签导航隐藏、`#hash` 硬跳回弹、后端接口 `403`（双保险）；
- `overview` 只读：看到脱敏连接串与统计；**`sources` 才能取令牌 / 看真实连接串 / 增删改**；
- 数据层再叠一层保护：命令行下面「字段级安全」的严格白名单 + `deny`/`mask`，每次查询进 SQL 审计。

> 远程访问且 `WebAuthLevel=None` 时，**只有本机免登录**，远程仍强制登录（防误配成全员裸奔）。

配合字段级安全（默认严格白名单）就是完整的出口管控：能调通哪些表/列由维护者登记，敏感列 `deny` / `mask`，
每次查询都进「SQL 审计」（谁、什么语句、命中/拦截了哪些列）。

## 安全

- **面板访问可锁死（`PanelLocalOnly`，默认关）**：打开后别的机器打不开面板（`/`、`/assets/*`、`/api/*` 一律 `403`），
  只能带令牌访问 `/Db/*`；日常靠账号 + 页签权限控制（见「部署模型」）。
- **写盘加密**：`Config/Proxy.toml` 里的连接串一律以 `enc:v1:`（AES-256-GCM）保存，密钥在 `Config/.key`（Unix 权限 600）；启动时自动把仍是明文的连接串加密迁移。
- 默认只读；写操作需在面板显式解除。
- 所有请求带令牌；审计里令牌只留指纹（`Ve-g****j5aA`）。
- WAF 全站生效，`/Db` 与 `/api/upgrade` 豁免攻击特征（分别由只读护栏与文件头校验兜底）。
- `AllowRemote=false` 时非回环请求直接拒绝；面板鉴权级别可设 `Full`（全部需 JWT）。

## 开发

```powershell
cargo test --offline                    # 12 单测 + 2 端到端
cargo clippy --all-targets --offline    # 0 告警
```

接续与记忆：见 `docs/项目接续指南.md`、`docs/项目记忆.md`、`docs/决策记录.md`。

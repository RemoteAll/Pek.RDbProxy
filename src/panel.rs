//! 面板与路由（对齐 Pek 系规范）。
//!
//! - 路由 / 中间件：`dhrust::net::router`（`Router` + `Ctx`）
//! - 面板鉴权：`dhrust::net::panel_auth`（`AuthLevel` / `bearer_token` / `client_ip` / `is_loopback`）
//! - 用户令牌：`pek_rpermissions::jwt`（HS256，与 C# NewLife `JwtBuilder` 互通）
//! - WAF：`pek_rwaf`（`admin_api` 预设；`/Db` 前缀豁免——SQL 文本不该做注入特征匹配）
//!
//! **注意**：`res/index.html` 是 `include_str!` 内嵌，改动后需 touch 本文件（或 `cargo clean -p pek-rdbproxy`）重编。

use std::sync::Arc;

use dhrust::net::http::{HttpOutcome, HttpResponse};
use dhrust::net::panel_auth::{AuthLevel, bearer_token, client_ip, is_loopback};
use dhrust::net::router::{Ctx, Next, Router, route};
use dhrust::net::static_files::StaticFiles;
use serde_json::{Map, Value, json};

use crate::config::DatabaseSection;
use crate::proxy::Proxy;

/// 面板静态资源（与 Pek.RAgent 同一套 `web/` 结构：骨架 HTML + app.css + app.js）。
fn statics() -> StaticFiles {
    StaticFiles::new("wwwroot")
        .embed(
            "/index.html",
            include_bytes!("../res/index.html"),
            "text/html; charset=utf-8",
        )
        .embed(
            "/assets/app.css",
            include_bytes!("../res/app.css"),
            "text/css; charset=utf-8",
        )
        .embed(
            "/assets/app.js",
            include_bytes!("../res/app.js"),
            "application/javascript; charset=utf-8",
        )
        .spa_fallback(true)
        .spa_excludes(&["/api", "/Db"])
}

/// 组装全部路由。
pub fn build_router(proxy: Arc<Proxy>) -> Router {
    let mut router = Router::new();

    // ── WAF：所有带网页的项目必须接；`/Db` 是机器协议（SQL 文本、无浏览器 UA），必须豁免
    let mut waf_config = pek_rwaf::WafConfig::admin_api();
    waf_config.skip_attack_prefixes = vec!["/Db".to_string(), "/api/upgrade".to_string()];
    let waf = pek_rwaf::Waf::load_with_default(proxy.base(), waf_config);
    waf.set_forced_skip_bot_prefixes(vec!["/Db".to_string()]);
    router.use_middleware(waf.middleware());

    // ── 面板仅限本机（`PanelLocalOnly`，默认开）
    //
    // 定位：本工具给开发做本地调测，真实连接串/字段策略不能让开发人员看到或改。
    // 所以只有 `/Db/*` 对远程开放（走数据源令牌），面板本身只接受回环访问。
    {
        let proxy = Arc::clone(&proxy);
        router.use_middleware(Arc::new(move |ctx: Ctx, next: Next| {
            let proxy = Arc::clone(&proxy);
            Box::pin(async move {
                let path = ctx.req.path.clone();
                if !path.starts_with("/Db") && proxy.server_config().panel_local_only {
                    let ip = client_ip(&ctx);
                    if !is_loopback(&ip) {
                        dhrust::warn!("面板被非本机访问拒绝：ip={ip} path={path}");
                        return HttpOutcome::Response(fail(
                            403,
                            "面板仅限本机访问（PanelLocalOnly=true）；远程请用令牌调用 /Db/*",
                        ));
                    }
                }
                next(ctx).await
            })
        }));
    }

    // ── 登录 / 登出（JWT）
    router.map_post("/api/login", login(Arc::clone(&proxy)));
    router.map_post("/api/logout", logout());

    // ── 面板 API（需鉴权 + 页签权限，见 `guard`）
    router.map_get("/api/status", guard(Arc::clone(&proxy), "", status));
    router.map_get("/api/audit", guard(Arc::clone(&proxy), "audit", audit));
    // 数据源写操作 + 凭据（令牌 / 真实连接串）需 `sources` 权限；只读概览只需登录
    router.map_post("/api/datasource", guard(Arc::clone(&proxy), "sources", save_source));
    router.map_post("/api/datasource/get", guard(Arc::clone(&proxy), "sources", get_source));
    router.map_post("/api/datasource/token", guard(Arc::clone(&proxy), "sources", get_token));
    router.map_post("/api/datasource/delete", guard(Arc::clone(&proxy), "sources", delete_source));
    router.map_post("/api/readonly", guard(Arc::clone(&proxy), "sources", set_read_only));
    router.map_post("/api/server", guard(Arc::clone(&proxy), "settings", save_server));
    router.map_post("/api/upgrade", guard(Arc::clone(&proxy), "settings", upgrade));
    router.map_get("/api/fields", guard(Arc::clone(&proxy), "fields", list_fields));
    router.map_post("/api/fields", guard(Arc::clone(&proxy), "fields", save_field));
    router.map_post("/api/fields/delete", guard(Arc::clone(&proxy), "fields", delete_field));
    router.map_post("/api/fields/mode", guard(Arc::clone(&proxy), "fields", set_field_mode));
    router.map_post("/api/fields/bulk", guard(Arc::clone(&proxy), "fields", bulk_fields));
    // 用户管理：仅内置管理员（`users` 不在可授予权限表里，表内用户永远拿不到）
    router.map_get("/api/users", guard(Arc::clone(&proxy), crate::admin::PERM_USERS, list_users));
    router.map_post("/api/user", guard(Arc::clone(&proxy), crate::admin::PERM_USERS, save_user));
    router.map_post("/api/user/delete", guard(Arc::clone(&proxy), crate::admin::PERM_USERS, remove_user));
    // 当前身份与可授权限（前端渲染菜单用：只要求登录）
    router.map_get("/api/me", me(Arc::clone(&proxy)));

    // ── 数据库代理协议（对齐 C# `DbController`；鉴权走各数据源令牌）
    router.map_post("/Db/{action}", db_call(Arc::clone(&proxy), "POST"));
    router.map_get("/Db/{action}", db_call(Arc::clone(&proxy), "GET"));

    // 静态资源：GET/HEAD → 面板文件（ETag/304 + SPA 深链接回退）；其他方法只允许真实文件
    let statics = statics();
    router.fallback(route(move |ctx: Ctx| {
        let statics = statics.clone();
        async move {
            let method_ok = ctx.req.method.eq_ignore_ascii_case("GET")
                || ctx.req.method.eq_ignore_ascii_case("HEAD");
            if method_ok {
                if let Some(response) = statics.try_serve_request(&ctx.req) {
                    return HttpOutcome::Response(response);
                }
            } else if let Some(response) = statics.try_serve_file(&ctx.req.path) {
                return HttpOutcome::Response(response);
            }
            HttpOutcome::Response(fail(404, "未知路由"))
        }
    }));
    router
}

/// 登录（用户名 + 密码 → JWT）。
///
/// 两类账号：
/// 1. **内置管理员**（配置文件 `WebUserName` / `WebPasswordHash`）——全部权限；
/// 2. **表内用户**（`Data/admin.db` 的 `RDbProxy_PanelUser`）——按所授页签权限，
///    凭据由内置管理员在「用户」页维护（密码只要求非空）。
fn login(proxy: Arc<Proxy>) -> dhrust::net::router::RouteHandler {
    route(move |ctx: Ctx| {
        let proxy = Arc::clone(&proxy);
        async move {
            let body = json_body(&ctx);
            let user = text(&body, "user");
            let password = text(&body, "password");
            if user.is_empty() || password.is_empty() {
                return HttpOutcome::Response(fail(400, "用户名与密码不能为空"));
            }
            let (permissions, builtin) = if proxy.authenticate(&user, &password) {
                (crate::admin::all_keys(), true)
            } else {
                match crate::admin::verify_login(proxy.base(), &user, &password) {
                    Ok(Some(panel_user)) => (panel_user.permissions, false),
                    Ok(None) => {
                        dhrust::warn!("面板登录失败：user={user} ip={}", client_ip(&ctx));
                        return HttpOutcome::Response(fail(401, "用户名或密码错误"));
                    }
                    Err(e) => {
                        dhrust::error!("面板用户库不可用：{e}");
                        return HttpOutcome::Response(fail(500, &format!("面板用户库不可用：{e}")));
                    }
                }
            };
            let jwt = match proxy.jwt() {
                Ok(jwt) => jwt,
                Err(e) => return HttpOutcome::Response(fail(500, &e)),
            };
            let mut claims = Map::new();
            claims.insert("sub".into(), json!(user));
            claims.insert("name".into(), json!(user));
            claims.insert("iss".into(), json!("Pek.RDbProxy"));
            claims.insert("from".into(), json!("panel"));
            claims.insert("perms".into(), json!(permissions));
            claims.insert("builtin".into(), json!(builtin));
            let token = jwt.encode(&claims);
            dhrust::info!(
                "面板登录成功：user={user} builtin={builtin} perms={} ip={}",
                permissions.join(","),
                client_ip(&ctx)
            );
            // 内置管理员仍是默认密码（admin）→ 自动换成随机强密码，新密码只在本次响应里返回
            let new_password = if builtin {
                match proxy.upgrade_weak_password() {
                    Ok(v) => v,
                    Err(e) => {
                        dhrust::warn!("默认密码自动升级失败：{e}");
                        None
                    }
                }
            } else {
                None
            };
            HttpOutcome::Response(ok(json!({
                "token": token,
                "user": user,
                "builtin": builtin,
                "permissions": permissions,
                "newPassword": new_password,
                "passwordRule": "密码不能为空（不强制复杂度）",
            })))
        }
    })
}

/// 登出（JWT 无状态：前端丢弃令牌即可）。
fn logout() -> dhrust::net::router::RouteHandler {
    route(|_ctx: Ctx| async { HttpOutcome::Response(ok(json!({ "ok": true }))) })
}

/// 当前登录身份 + 可授权限（前端据此隐藏无权限页签）。
fn me(proxy: Arc<Proxy>) -> dhrust::net::router::RouteHandler {
    route(move |ctx: Ctx| {
        let proxy = Arc::clone(&proxy);
        async move {
            match require(&proxy, &ctx) {
                Ok(who) => {
                    let allowed: Vec<Value> = crate::admin::ALL_PERMISSIONS
                        .iter()
                        .filter(|(key, _)| who.builtin || who.perms.iter().any(|p| p == key))
                        .map(|(key, name)| json!({ "key": key, "name": name }))
                        .collect();
                    HttpOutcome::Response(ok(json!({
                        "user": who.user,
                        "builtin": who.builtin,
                        "permissions": who.perms,
                        "allowed": allowed,
                    })))
                }
                Err(resp) => HttpOutcome::Response(resp),
            }
        }
    })
}

/// 面板主体（登录身份）。
struct Principal {
    user: String,
    perms: Vec<String>,
    builtin: bool,
}

/// 包装一个需要「登录 + 指定页签权限」的处理函数（`perm` 为空表示只要登录）。
fn guard(
    proxy: Arc<Proxy>,
    perm: &'static str,
    handler: fn(&Proxy, &Ctx) -> HttpResponse,
) -> dhrust::net::router::RouteHandler {
    route(move |ctx: Ctx| {
        let proxy = Arc::clone(&proxy);
        async move {
            match require(&proxy, &ctx) {
                Ok(who) => {
                    if perm.is_empty() || who.builtin || who.perms.iter().any(|p| p == perm) {
                        HttpOutcome::Response(handler(&proxy, &ctx))
                    } else {
                        dhrust::warn!(
                            "面板权限不足：user={} perm={perm} path={} ip={}",
                            who.user,
                            ctx.req.path,
                            client_ip(&ctx)
                        );
                        HttpOutcome::Response(fail(
                            403,
                            &format!("没有权限：{}（{perm}）", crate::admin::permission_name(perm)),
                        ))
                    }
                }
                Err(resp) => HttpOutcome::Response(resp),
            }
        }
    })
}

/// 面板鉴权（对齐 Pek.RAgent）：
/// 令牌有效（JWT）→ 放行；否则按 `WebAuthLevel`：None 全放行 / LocalOnly 本机放行 / Full 必须令牌。
///
/// 免登录的本机（None / LocalOnly + 回环）按**内置管理员**处理——本机能改配置就能管理用户。
fn require(proxy: &Proxy, ctx: &Ctx) -> Result<Principal, HttpResponse> {
    if let Some(token) = bearer_token(ctx)
        && let Ok(jwt) = proxy.jwt()
        && let Ok(claims) = jwt.validate(&token)
    {
        let user = claims
            .get("sub")
            .and_then(Value::as_str)
            .unwrap_or("admin")
            .to_string();
        let builtin = claims
            .get("builtin")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let perms = claims
            .get("perms")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        return Ok(Principal {
            user,
            perms,
            builtin,
        });
    }
    let server = proxy.server_config();
    let level = AuthLevel::parse(&server.web_auth_level);
    let ip = client_ip(ctx);
    match level {
        // `None` 只对**本机**免鉴权：已允许远程访问时，远程仍须登录（防误配成全员裸奔）
        AuthLevel::None if is_loopback(&ip) => Ok(Principal {
            user: "anonymous".into(),
            perms: crate::admin::all_keys(),
            builtin: true,
        }),
        AuthLevel::LocalOnly if is_loopback(&ip) => Ok(Principal {
            user: "local".into(),
            perms: crate::admin::all_keys(),
            builtin: true,
        }),
        _ => Err(fail(401, "需要登录")),
    }
}

/// `GET /api/status`：运行时长 + 服务端设置 + 数据源统计。
fn status(proxy: &Proxy, _ctx: &Ctx) -> HttpResponse {
    let server = proxy.server_config();
    let sources: Vec<Value> = proxy
        .source_list()
        .iter()
        .map(source_json)
        .collect();
    ok(json!({
        "version": crate::VERSION,
        "uptimeSecs": proxy.uptime_secs(),
        "server": {
            "host": server.host,
            "port": server.port,
            "allowRemote": server.allow_remote,
            "panelLocalOnly": server.panel_local_only,
            "webUserName": server.web_user_name,
            "webAuthLevel": server.web_auth_level,
            "defaultPassword": proxy.uses_default_password(),
            "firewallNote": proxy.firewall_note(),
        },
        "sources": sources,
    }))
}

/// `GET /api/audit?n=`：最近审计记录。
fn audit(proxy: &Proxy, ctx: &Ctx) -> HttpResponse {
    let limit = ctx
        .query_value("n")
        .and_then(|text| text.parse::<usize>().ok())
        .unwrap_or(100)
        .min(crate::proxy::AUDIT_CAPACITY);
    let entries: Vec<Value> = proxy.audit_entries(limit).iter().map(|e| e.to_json()).collect();
    ok(json!(entries))
}

/// `POST /api/datasource`：新增或更新数据源（同时写回配置）。
fn save_source(proxy: &Proxy, ctx: &Ctx) -> HttpResponse {
    let body = json_body(ctx);
    if body.as_object().is_none_or(|map| map.is_empty()) {
        return fail(
            400,
            &format!(
                "请求体不是合法 JSON 对象（收到 {} 字节，Content-Type={}）",
                ctx.req.body.len(),
                ctx.req.header("content-type").unwrap_or("-")
            ),
        );
    }
    let section = DatabaseSection {
        name: text(&body, "name"),
        connection: text(&body, "connection"),
        token: text(&body, "token"),
        read_only: body.get("readOnly").and_then(Value::as_bool).unwrap_or(true),
        description: text(&body, "description"),
    };
    match proxy.upsert_source(section) {
        Ok(view) => ok(source_json(&view)),
        Err(e) => fail(400, &e),
    }
}

/// `POST /api/datasource/token`：取数据源令牌（面板「复制项目侧连接串」用）。
fn get_token(proxy: &Proxy, ctx: &Ctx) -> HttpResponse {
    let body = json_body(ctx);
    let name = text(&body, "name");
    match proxy.source_token(&name) {
        Ok(token) => ok(json!({ "name": name, "token": token })),
        Err(e) => fail(404, &e),
    }
}

/// `POST /api/datasource/get`：取完整连接串（面板「编辑」回填；仅鉴权后可访问）。
fn get_source(proxy: &Proxy, ctx: &Ctx) -> HttpResponse {
    let body = json_body(ctx);
    let name = text(&body, "name");
    match proxy.source_connection(&name) {
        Ok(connection) => ok(json!({ "name": name, "connection": connection })),
        Err(e) => fail(404, &e),
    }
}

/// `POST /api/datasource/delete`：删除数据源。
fn delete_source(proxy: &Proxy, ctx: &Ctx) -> HttpResponse {
    let body = json_body(ctx);
    let name = text(&body, "name");
    match proxy.remove_source(&name) {
        Ok(()) => ok(json!({ "name": name })),
        Err(e) => fail(400, &e),
    }
}

/// `POST /api/upgrade`：上传新程序到 `Update/`（约 10 秒后由热升级监视器原子替换并重启）。
fn upgrade(proxy: &Proxy, ctx: &Ctx) -> HttpResponse {
    const MAX_SIZE: usize = 64 * 1024 * 1024;
    let body = &ctx.req.body;
    if body.is_empty() {
        return fail(400, "没有收到文件内容（请选择程序文件再上传）");
    }
    if body.len() > MAX_SIZE {
        return fail(413, "文件过大（上限 64MB）");
    }
    if body.len() < 4096 {
        return fail(400, "文件太小，不是有效的可执行程序");
    }
    let is_pe = body[0] == b'M' && body[1] == b'Z';
    let is_elf = body.starts_with(b"\x7fELF");
    if !is_pe && !is_elf {
        return fail(400, "不是有效的可执行文件（需要 PE 或 ELF 文件头）");
    }
    let dir = proxy.base().join("Update");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return fail(500, &format!("创建 Update 目录失败：{e}"));
    }
    let name = format!(
        "pek-rdbproxy-{}.new",
        chrono::Local::now().format("%Y%m%d%H%M%S")
    );
    let path = dir.join(&name);
    if let Err(e) = std::fs::write(&path, body) {
        return fail(500, &format!("写入失败：{e}"));
    }
    dhrust::info!(
        "收到在线升级包：{}（{} 字节），约 10 秒后自动替换并重启",
        name,
        body.len()
    );
    ok(json!({
        "file": name,
        "bytes": body.len(),
        "note": "已写入 Update/，约 10 秒后自动替换并重启（面板会短暂断开）"
    }))
}

/// `GET /api/fields`：字段策略（模式 + 规则 + 未登记字段观测）。
fn list_fields(proxy: &Proxy, _ctx: &Ctx) -> HttpResponse {
    let policy = proxy.policy_snapshot();
    let rules: Vec<Value> = policy
        .rules
        .iter()
        .map(|r| {
            json!({
                "db": r.db,
                "column": r.column,
                "action": r.action.as_str(),
                "note": r.note,
            })
        })
        .collect();
    ok(json!({
        "mode": policy.mode.as_str(),
        "rules": rules,
        "unknown": proxy.unknown_fields(),
    }))
}

/// `POST /api/fields`：新增/更新一条列策略。
fn save_field(proxy: &Proxy, ctx: &Ctx) -> HttpResponse {
    let body = json_body(ctx);
    let db = text(&body, "db");
    let column = text(&body, "column");
    if db.is_empty() || column.is_empty() {
        return fail(400, "数据源与列名不能为空");
    }
    let mut policy = proxy.policy_snapshot();
    policy.upsert(crate::policy::Rule {
        db,
        column,
        action: crate::policy::Action::parse(&text(&body, "action")),
        note: text(&body, "note"),
    });
    match proxy.save_policy(policy) {
        Ok(()) => ok(json!({ "ok": true })),
        Err(e) => fail(500, &e),
    }
}

/// `POST /api/fields/delete`：删除一条列策略。
fn delete_field(proxy: &Proxy, ctx: &Ctx) -> HttpResponse {
    let body = json_body(ctx);
    let db = text(&body, "db");
    let column = text(&body, "column");
    let mut policy = proxy.policy_snapshot();
    if !policy.remove(&db, &column) {
        return fail(404, "规则不存在");
    }
    match proxy.save_policy(policy) {
        Ok(()) => ok(json!({ "ok": true })),
        Err(e) => fail(500, &e),
    }
}

/// `POST /api/fields/mode`：切换生效模式（`audit` / `enforce`）。
fn set_field_mode(proxy: &Proxy, ctx: &Ctx) -> HttpResponse {
    let body = json_body(ctx);
    let mut policy = proxy.policy_snapshot();
    policy.mode = crate::policy::Mode::parse(&text(&body, "mode"));
    let mode = policy.mode.as_str();
    match proxy.save_policy(policy) {
        Ok(()) => ok(json!({ "mode": mode })),
        Err(e) => fail(500, &e),
    }
}

/// `POST /api/fields/bulk`：批量登记（同数据源、同动作）。
///
/// `columns` 为空时取「审计里观测到的未登记列」（`UnknownFields`）——
/// 白名单模式（未登记列默认不返回）下用它一键初始化 `allow`。
fn bulk_fields(proxy: &Proxy, ctx: &Ctx) -> HttpResponse {
    let body = json_body(ctx);
    let db = text(&body, "db");
    let action = crate::policy::Action::parse(&text(&body, "action"));
    let note = text(&body, "note");
    if db.is_empty() {
        return fail(400, "数据源不能为空");
    }
    let mut columns: Vec<String> = body
        .get("columns")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    if columns.is_empty() {
        columns = proxy
            .unknown_fields()
            .get(&db)
            .map(|map| map.keys().cloned().collect())
            .unwrap_or_default();
    }
    if columns.is_empty() {
        return fail(400, "没有可登记的列（先跑一次查询，或显式传 columns）");
    }
    let count = columns.len();
    let mut policy = proxy.policy_snapshot();
    for column in columns {
        policy.upsert(crate::policy::Rule {
            db: db.clone(),
            column,
            action,
            note: note.clone(),
        });
    }
    match proxy.save_policy(policy) {
        Ok(()) => {
            dhrust::info!("字段策略批量登记：db={db} action={} 共 {count} 列", action.as_str());
            ok(json!({ "count": count }))
        }
        Err(e) => fail(500, &e),
    }
}

// ————— 面板用户（仅内置管理员；`users` 不在可授予权限表内） —————
/// `GET /api/users`：用户列表 + 可授权限（内置管理员以虚拟条目展示）。
fn list_users(proxy: &Proxy, _ctx: &Ctx) -> HttpResponse {
    let mut data = match crate::admin::list_users_json(proxy.base()) {
        Ok(data) => data,
        Err(e) => return fail(500, &e),
    };
    if let Some(obj) = data.as_object_mut() {
        let server = proxy.server_config();
        let keys: Vec<Value> = crate::admin::ALL_PERMISSIONS
            .iter()
            .map(|(k, _)| json!(k))
            .collect();
        let names: Vec<Value> = crate::admin::ALL_PERMISSIONS
            .iter()
            .map(|(_, n)| json!(n))
            .collect();
        obj.insert(
            "builtinAdmin".to_string(),
            json!({
                "userName": server.web_user_name,
                "permissions": keys,
                "permissionNames": names,
                "enabled": true,
                "remark": "内置管理员（Config/Proxy.toml 的 WebUserName / WebPassword；在用户页点「编辑」修改）",
            }),
        );
    }
    ok(data)
}

/// `POST /api/user`：保存用户（新建/更新；`originalName` 非空且与 `name` 不同表示改名）。
///
/// `builtin: true` 时改的是**内置管理员**（`Config/Proxy.toml` 的 `WebUserName` / `WebPasswordHash`）。
fn save_user(proxy: &Proxy, ctx: &Ctx) -> HttpResponse {
    let body = json_body(ctx);
    let name = text(&body, "name");
    if name.is_empty() {
        return fail(400, "用户名不能为空");
    }
    // 内置管理员：只改用户名/密码，走服务端配置（恒有全部权限、不可删除）
    if body.get("builtin").and_then(Value::as_bool).unwrap_or(false) {
        let password = text(&body, "password");
        let patch = crate::proxy::ServerPatch {
            user_name: Some(name.clone()),
            password: if password.trim().is_empty() {
                None
            } else {
                Some(password)
            },
            ..Default::default()
        };
        return match proxy.update_server(patch) {
            Ok(server) => {
                dhrust::info!("内置管理员凭据已更新：{}", server.web_user_name);
                ok(json!({ "ok": true, "userName": server.web_user_name }))
            }
            Err(e) => fail(400, &e),
        };
    }
    let original = text(&body, "originalName");
    let password = text(&body, "password");
    let permissions: Vec<String> = body
        .get("permissions")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let enabled = body.get("enabled").and_then(Value::as_bool).unwrap_or(true);
    let remark = text(&body, "remark");
    let password = if password.trim().is_empty() {
        None
    } else {
        Some(password.as_str())
    };
    if let Err(e) =
        crate::admin::save_user(proxy.base(), &name, password, &permissions, enabled, &remark)
    {
        return fail(400, &e);
    }
    // 改名：先保存新名字，成功后再删旧用户（失败也不影响已保存的新用户）
    if !original.is_empty()
        && !original.eq_ignore_ascii_case(&name)
        && let Err(e) = crate::admin::delete_user(proxy.base(), &original)
    {
        dhrust::warn!("改名后删除旧用户失败：{original} → {name}：{e}");
    }
    dhrust::info!("面板用户已保存：{name} perms={}", permissions.join(","));
    ok(json!({ "ok": true }))
}

/// `POST /api/user/delete`：删除用户。
fn remove_user(proxy: &Proxy, ctx: &Ctx) -> HttpResponse {
    let body = json_body(ctx);
    let name = text(&body, "name");
    if name.is_empty() {
        return fail(400, "用户名不能为空");
    }
    match crate::admin::delete_user(proxy.base(), &name) {
        Ok(()) => {
            dhrust::info!("面板用户已删除：{name}");
            ok(json!({ "ok": true }))
        }
        Err(e) => fail(400, &e),
    }
}

/// `POST /api/readonly`：切换只读（立即持久化）。
fn set_read_only(proxy: &Proxy, ctx: &Ctx) -> HttpResponse {
    let body = json_body(ctx);
    let name = text(&body, "db");
    let read_only = body.get("readOnly").and_then(Value::as_bool).unwrap_or(true);
    match proxy.set_read_only(&name, read_only) {
        Ok(view) => ok(source_json(&view)),
        Err(e) => fail(400, &e),
    }
}

/// `POST /api/server`：保存服务端设置（端口变更需重启）。
fn save_server(proxy: &Proxy, ctx: &Ctx) -> HttpResponse {
    let body = json_body(ctx);
    let patch = crate::proxy::ServerPatch {
        port: body.get("port").and_then(Value::as_u64).map(|v| v as u16),
        user_name: body.get("userName").and_then(Value::as_str).map(str::to_string),
        password: body.get("password").and_then(Value::as_str).map(str::to_string),
        auth_level: body.get("authLevel").and_then(Value::as_str).map(str::to_string),
        allow_remote: body.get("allowRemote").and_then(Value::as_bool),
        panel_local_only: body.get("panelLocalOnly").and_then(Value::as_bool),
    };
    match proxy.update_server(patch) {
        Ok(server) => ok(json!({
            "port": server.port,
            "webUserName": server.web_user_name,
            "webAuthLevel": server.web_auth_level,
            "allowRemote": server.allow_remote,
            "panelLocalOnly": server.panel_local_only,
            "rebound": format!("{}:{}", server.host, server.port),
            "restartRequired": false,
            "firewallNote": proxy.firewall_note(),
        })),
        Err(e) => fail(400, &e),
    }
}

/// `/Db/{action}`：转发到代理核心。
fn db_call(proxy: Arc<Proxy>, method: &'static str) -> dhrust::net::router::RouteHandler {
    route(move |ctx: Ctx| {
        let proxy = Arc::clone(&proxy);
        async move {
            let action = ctx.param("action").unwrap_or_default().to_string();
            let body = ctx.req.body.to_vec();
            let query = ctx.req.query.clone();
            let response = proxy.handle_db(&action, method, &body, &query);
            HttpOutcome::Response(response)
        }
    })
}

/// 数据源 JSON（面板用）。
fn source_json(view: &crate::proxy::SourceView) -> Value {
    json!({
        "name": view.name,
        "kind": view.kind,
        "description": view.description,
        "readOnly": view.read_only,
        "tokenSet": view.token_set,
        "available": view.available,
        "error": view.error,
        "connection": view.connection,
        "requests": view.requests,
        "failures": view.failures,
        "avgUs": view.avg_us,
        "lastError": view.last_error,
    })
}

/// 解析 JSON 请求体（非法返回空对象）。
fn json_body(ctx: &Ctx) -> Value {
    serde_json::from_slice(&ctx.req.body).unwrap_or_else(|_| Value::Object(Map::new()))
}

/// 取字符串字段。
fn text(body: &Value, key: &str) -> String {
    body.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// 面板成功信封。
fn ok(data: Value) -> HttpResponse {
    HttpResponse::json(200, json!({ "ok": true, "data": data }).to_string())
}

/// 面板错误信封。
fn fail(status: u16, message: &str) -> HttpResponse {
    HttpResponse::json(status, json!({ "ok": false, "message": message }).to_string())
}

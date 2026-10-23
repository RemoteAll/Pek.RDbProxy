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
use dhrust::net::router::{Ctx, Router, route};
use serde_json::{Map, Value, json};

use crate::config::DatabaseSection;
use crate::proxy::Proxy;

/// 面板页面（单文件 HTML，无外部依赖）。
pub const INDEX_HTML: &str = include_str!("../res/index.html");

/// 组装全部路由。
pub fn build_router(proxy: Arc<Proxy>) -> Router {
    let mut router = Router::new();

    // ── WAF：所有带网页的项目必须接；`/Db` 是机器协议（SQL 文本、无浏览器 UA），必须豁免
    let mut waf_config = pek_rwaf::WafConfig::admin_api();
    waf_config.skip_attack_prefixes = vec!["/Db".to_string()];
    let waf = pek_rwaf::Waf::load_with_default(proxy.base(), waf_config);
    waf.set_forced_skip_bot_prefixes(vec!["/Db".to_string()]);
    router.use_middleware(waf.middleware());

    // ── 面板页面
    for path in ["/", "/index.html"] {
        router.map_get(path, page());
    }

    // ── 登录 / 登出（JWT）
    router.map_post("/api/login", login(Arc::clone(&proxy)));
    router.map_post("/api/logout", logout());

    // ── 面板 API（需鉴权，见 `require`）
    router.map_get("/api/status", guard(Arc::clone(&proxy), status));
    router.map_get("/api/audit", guard(Arc::clone(&proxy), audit));
    router.map_post("/api/datasource", guard(Arc::clone(&proxy), save_source));
    router.map_post("/api/datasource/delete", guard(Arc::clone(&proxy), delete_source));
    router.map_post("/api/readonly", guard(Arc::clone(&proxy), set_read_only));
    router.map_post("/api/server", guard(Arc::clone(&proxy), save_server));

    // ── 数据库代理协议（对齐 C# `DbController`；鉴权走各数据源令牌）
    router.map_post("/Db/{action}", db_call(Arc::clone(&proxy), "POST"));
    router.map_get("/Db/{action}", db_call(Arc::clone(&proxy), "GET"));

    router.fallback(route(|_ctx| async {
        HttpOutcome::Response(fail(404, "未知路由"))
    }));
    router
}

/// 页面。
fn page() -> dhrust::net::router::RouteHandler {
    route(|_ctx: Ctx| async {
        HttpOutcome::Response(HttpResponse::bytes(
            200,
            "text/html; charset=utf-8",
            INDEX_HTML.as_bytes().to_vec(),
        ))
    })
}

/// 登录（用户名 + 密码 → JWT）。
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
            if !proxy.authenticate(&user, &password) {
                dhrust::warn!("面板登录失败：user={user} ip={}", client_ip(&ctx));
                return HttpOutcome::Response(fail(401, "用户名或密码错误"));
            }
            let jwt = match proxy.jwt() {
                Ok(jwt) => jwt,
                Err(e) => return HttpOutcome::Response(fail(500, &e)),
            };
            let mut claims = Map::new();
            claims.insert("sub".into(), json!(user));
            claims.insert("name".into(), json!(user));
            claims.insert("iss".into(), json!("Pek.RDbProxy"));
            claims.insert("from".into(), json!("panel"));
            let token = jwt.encode(&claims);
            dhrust::info!("面板登录成功：user={user} ip={}", client_ip(&ctx));
            HttpOutcome::Response(ok(json!({ "token": token, "user": user })))
        }
    })
}

/// 登出（JWT 无状态：前端丢弃令牌即可）。
fn logout() -> dhrust::net::router::RouteHandler {
    route(|_ctx: Ctx| async { HttpOutcome::Response(ok(json!({ "ok": true }))) })
}

/// 包装一个需要面板鉴权的处理函数（用 `fn` 指针，无状态）。
fn guard(
    proxy: Arc<Proxy>,
    handler: fn(&Proxy, &Ctx) -> HttpResponse,
) -> dhrust::net::router::RouteHandler {
    route(move |ctx: Ctx| {
        let proxy = Arc::clone(&proxy);
        async move {
            match require(&proxy, &ctx) {
                Ok(_user) => HttpOutcome::Response(handler(&proxy, &ctx)),
                Err(resp) => HttpOutcome::Response(resp),
            }
        }
    })
}

/// 面板鉴权（对齐 Pek.RAgent）：
/// 令牌有效（JWT）→ 放行；否则按 `WebAuthLevel`：None 全放行 / LocalOnly 本机放行 / Full 必须令牌。
fn require(proxy: &Proxy, ctx: &Ctx) -> Result<String, HttpResponse> {
    if let Some(token) = bearer_token(ctx)
        && let Ok(jwt) = proxy.jwt()
        && let Ok(claims) = jwt.validate(&token)
    {
        let user = claims
            .get("sub")
            .and_then(Value::as_str)
            .unwrap_or("admin")
            .to_string();
        return Ok(user);
    }
    let server = proxy.server_config();
    let level = AuthLevel::parse(&server.web_auth_level);
    let ip = client_ip(ctx);
    match level {
        AuthLevel::None => Ok("anonymous".into()),
        AuthLevel::LocalOnly if is_loopback(&ip) => Ok("local".into()),
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
        "uptimeSecs": proxy.uptime_secs(),
        "server": {
            "host": server.host,
            "port": server.port,
            "allowRemote": server.allow_remote,
            "webUserName": server.web_user_name,
            "webAuthLevel": server.web_auth_level,
            "defaultPassword": server.web_password_hash.is_empty(),
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

/// `POST /api/datasource/delete`：删除数据源。
fn delete_source(proxy: &Proxy, ctx: &Ctx) -> HttpResponse {
    let body = json_body(ctx);
    let name = text(&body, "name");
    match proxy.remove_source(&name) {
        Ok(()) => ok(json!({ "name": name })),
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
    };
    let old_port = proxy.server_config().port;
    match proxy.update_server(patch) {
        Ok(server) => ok(json!({
            "port": server.port,
            "webUserName": server.web_user_name,
            "webAuthLevel": server.web_auth_level,
            "allowRemote": server.allow_remote,
            "restartRequired": server.port != old_port,
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

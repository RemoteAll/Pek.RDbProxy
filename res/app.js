/* Pek.RDbProxy 管理面板前端
 * 结构与样式完全复用 Pek.RAgent 的 web/app.css（同一套类名与排版），
 * 仅内容区按自身业务（数据源 / 审计 / 设置）渲染。
 */

const $ = (id) => document.getElementById(id);
const TOKEN_KEY = 'rdbproxy_token';
let token = '';
let status = null;
let fields = null;
let me = null;
let usersData = null;
let pollTimer = null;

/* ── 通用 ───────────────────────────────────────────── */

function toast(msg, type = 'info') {
  const t = $('toast');
  t.textContent = msg;
  t.className = 'toast ' + type + ' show';
  clearTimeout(t._tid);
  t._tid = setTimeout(() => t.classList.remove('show'), 3500);
}
function showToast(msg, type) { toast(msg, type); }

function esc(s) {
  return String(s == null ? '' : s)
    .replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;').replace(/'/g, '&#39;');
}
function fmtUptime(sec) {
  sec = Math.max(0, sec | 0);
  const d = Math.floor(sec / 86400), h = Math.floor((sec % 86400) / 3600);
  const m = Math.floor((sec % 3600) / 60), s = sec % 60;
  if (d) return `${d} 天 ${h} 小时`;
  if (h) return `${h} 小时 ${m} 分`;
  if (m) return `${m} 分 ${s} 秒`;
  return `${s} 秒`;
}
function fmtUs(us) {
  if (!us) return '0 µs';
  if (us < 1000) return us + ' µs';
  return (us / 1000).toFixed(2) + ' ms';
}

let _lastElapsed = '--';
function setSrvElapsed(ms) {
  _lastElapsed = ms;
  const f = $('srv-foot');
  if (f) f.textContent = `Pek.RDbProxy · 服务端处理耗时：${ms} ms（最近一次请求）`;
}

let _modalCb = null;
function showModal(title, body, confirmText, cb) {
  $('modal-title').textContent = title;
  $('modal-body').innerHTML = body;
  $('modal-confirm-btn').textContent = confirmText || '确认';
  $('modal-overlay').classList.remove('hidden');
  _modalCb = cb;
}
function closeModal() { $('modal-overlay').classList.add('hidden'); _modalCb = null; }
function confirmModal() { const cb = _modalCb; closeModal(); if (cb) cb(); }

/* ── 请求 ───────────────────────────────────────────── */

async function api(url, opt = {}) {
  const h = Object.assign({}, opt.headers || {});
  if (token) h['Authorization'] = 'Bearer ' + token;
  if (opt.body) h['Content-Type'] = 'application/json';
  const r = await fetch(url, Object.assign({}, opt, { headers: h }));
  const el = r.headers.get('X-Elapsed-Ms');
  if (el) setSrvElapsed(el);
  let j = null;
  try { j = await r.json(); } catch (e) { j = { ok: false, message: '响应不是 JSON' }; }
  if (r.status === 401) {
    if ($('login').style.display !== 'none') {
      // 登录页：交给调用方显示错误
      throw new Error(j && j.message ? j.message : '用户名或密码错误');
    }
    logout();
    toast('登录已过期，请重新登录', 'error');
    throw new Error('Unauthorized');
  }
  return j;
}

function unwrap(j) {
  if (j && j.ok) return j.data;
  throw new Error((j && j.message) || '请求失败');
}

/* ── 登录 ───────────────────────────────────────────── */

function readToken() {
  const t = localStorage.getItem(TOKEN_KEY) || sessionStorage.getItem(TOKEN_KEY) || '';
  return t;
}
function storeToken(t, remember) {
  token = t;
  try {
    if (remember) { localStorage.setItem(TOKEN_KEY, t); sessionStorage.removeItem(TOKEN_KEY); }
    else { sessionStorage.setItem(TOKEN_KEY, t); localStorage.removeItem(TOKEN_KEY); }
  } catch (e) { /* 隐私模式忽略 */ }
}
function clearToken() {
  token = '';
  try { localStorage.removeItem(TOKEN_KEY); sessionStorage.removeItem(TOKEN_KEY); } catch (e) {}
}

async function doLogin() {
  const user = $('luser').value.trim();
  const pass = $('lpass').value;
  const err = $('lerr');
  err.classList.remove('show');
  if (!user || !pass) { err.textContent = '请输入用户名与密码'; err.classList.add('show'); return; }
  try {
    const r = await fetch('/api/login', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ user, password: pass }),
    });
    const el = r.headers.get('X-Elapsed-Ms'); if (el) setSrvElapsed(el);
    const j = await r.json();
    if (!r.ok || !j.ok) throw new Error((j && j.message) || '登录失败');
    storeToken(j.data.token, $('lremember').checked);
    enterApp();
    if (j.data.newPassword) {
      const pwd = j.data.newPassword;
      showModal('⚠ 默认密码已自动更换', `
        <p>检测到面板仍在使用默认密码 <b>admin</b>，已按强度规则自动更换为强密码：</p>
        <div class="font-mono" style="word-break:break-all;background:var(--surface-2);padding:12px;border-radius:8px;font-size:15px;margin:10px 0">${esc(pwd)}</div>
        <p>请立即复制保存（<b>只显示这一次</b>），之后可在「设置」里改成你自己记得的强密码。</p>
        <div style="display:flex;gap:8px;margin-top:10px">
          <button class="btn btn-primary btn-sm" onclick="navigator.clipboard.writeText('${esc(pwd)}')">复制新密码</button>
        </div>`,
        '我已保存', null);
    }
  } catch (e) {
    err.textContent = e.message || '登录失败';
    err.classList.add('show');
  }
}

function logout() {
  clearToken();
  if (pollTimer) { clearInterval(pollTimer); pollTimer = null; }
  $('app').style.display = 'none';
  $('login').style.display = '';
  $('lpass').value = '';
}

function enterApp() {
  $('login').style.display = 'none';
  $('app').style.display = 'block';
  loadMe().then(() => switchPanel(panelFromHash()));
  loadStatus();
  if (pollTimer) clearInterval(pollTimer);
  // 概览页 3 秒刷新（对齐 RAgent 面板节奏；其它页签进入时按需拉取）
  pollTimer = setInterval(() => {
    const cur = document.querySelector('nav a.active');
    if (cur && cur.dataset.panel === 'overview') loadStatus(true);
  }, 3000);
}

/* ── 页签 ───────────────────────────────────────────── */

function panelFromHash() {
  const h = (location.hash || '').replace('#', '');
  return ['overview', 'audit', 'fields', 'users', 'settings'].includes(h) ? h : 'overview';
}

function canPerm(perm) {
  if (!me) return true;
  if (me.builtin) return true;
  return (me.permissions || []).includes(perm);
}

function panelAllowed(name) {
  if (!me) return true;
  if (me.builtin) return true;
  if (name === 'users') return false;
  return (me.permissions || []).includes(name);
}

function renderNav() {
  document.querySelectorAll('nav a[data-panel]').forEach(a => {
    a.style.display = panelAllowed(a.dataset.panel) ? '' : 'none';
  });
  const cur = document.querySelector('nav a.active');
  if (cur && cur.style.display === 'none') {
    const first = [...document.querySelectorAll('nav a[data-panel]')].find(a => a.style.display !== 'none');
    if (first) switchPanel(first.dataset.panel);
  }
}

async function loadMe() {
  try { me = unwrap(await api('/api/me')); } catch (e) { me = null; }
  renderNav();
}

function switchPanel(name) {
  if (!panelAllowed(name)) name = 'overview';
  document.querySelectorAll('nav a').forEach(a => a.classList.remove('active'));
  const link = document.querySelector(`nav a[data-panel="${name}"]`);
  if (link) link.classList.add('active');
  document.querySelectorAll('.panel').forEach(p => p.classList.remove('active'));
  const panel = $('panel-' + name);
  if (panel) panel.classList.add('active');
  if (history.replaceState) history.replaceState(null, '', '#' + name);

  const loaders = { overview: loadStatus, audit: loadAudit, fields: loadFields, users: loadUsers, settings: loadSettings };
  if (loaders[name]) loaders[name]();
}

document.addEventListener('click', (e) => {
  const a = e.target.closest('nav a[data-panel]');
  if (a) switchPanel(a.dataset.panel);
});

/* ── 概览：数据源 ───────────────────────────────────── */

async function loadStatus(silent) {
  try {
    status = unwrap(await api('/api/status'));
  } catch (e) {
    if (!silent) toast(e.message, 'error');
    return;
  }
  renderHeader();
  renderBanner();
  renderOverview();
}

function renderHeader() {
  const s = status.server || {};
  const meta = $('header-meta');
  if (meta) {
    meta.textContent = `v${status.version || '0.1.0'} · 运行 ${fmtUptime(status.uptimeSecs)} · ${s.allowRemote ? '允许远程' : '仅本机'}`;
  }
  const dot = $('status-dot-header');
  if (dot) dot.title = `${s.host}:${s.port}`;
}

function renderBanner() {
  const s = status.server || {};
  const el = $('sec-banner');
  if (!el) return;
  if (s.defaultPassword) {
    el.classList.remove('hidden');
    el.textContent = '⚠ 面板仍在使用默认密码（admin/admin），请立即到「设置」修改'
      + (s.allowRemote ? '；当前已允许远程访问，存在被扫描利用风险' : '');  } else {
    el.classList.add('hidden');
  }
}

function renderOverview() {
  const sources = status.sources || [];
  const readOnly = sources.filter(s => s.readOnly).length;
  const reqs = sources.reduce((a, s) => a + (s.requests || 0), 0);
  const fails = sources.reduce((a, s) => a + (s.failures || 0), 0);
  const avg = (() => {
    const tot = sources.reduce((a, s) => a + (s.requests || 0) * (s.avgUs || 0), 0);
    return reqs ? Math.round(tot / reqs) : 0;
  })();

  let html = `
  <div class="stats-grid mb-16">
    <div class="stat"><div class="stat-val">${sources.length}</div><div class="stat-lbl">数据源</div></div>
    <div class="stat"><div class="stat-val">${readOnly}</div><div class="stat-lbl">只读</div></div>
    <div class="stat"><div class="stat-val">${reqs}</div><div class="stat-lbl">累计请求</div></div>
    <div class="stat"><div class="stat-val ${fails ? 'text-red' : ''}">${fails}</div><div class="stat-lbl">失败</div></div>
    <div class="stat"><div class="stat-val">${fmtUs(avg)}</div><div class="stat-lbl">平均耗时</div></div>
  </div>

  <div class="card glass-card">
    <h2>🗄 数据源
      <span style="margin-left:auto;display:flex;gap:8px">
        ${canPerm('sources') ? '<button class="btn btn-primary btn-sm" onclick="editSource(null)">+ 新增数据源</button>' : ''}
        <button class="btn btn-secondary btn-sm" onclick="loadStatus()">刷新</button>
      </span>
    </h2>`;

  if (!sources.length) {
    html += `<div class="empty-state"><div class="empty-icon">🗄</div><div>还没有数据源，点「+ 新增数据源」添加</div></div>`;
  } else {
    html += `<div class="ds-grid">`;
    for (const s of sources) {
      const badge = s.available
        ? '<span class="badge badge-green">可用</span>'
        : `<span class="badge badge-red" title="${esc(s.error)}">不可用</span>`;
      html += `<div class="glass-card ds-card">
        <div class="flex-between" style="margin-bottom:10px">
          <div style="min-width:0">
            <div class="ds-name">${esc(s.name)}</div>
            <div class="text-muted" style="font-size:11px;margin-top:2px">${esc(s.kind)}${s.description ? ' · ' + esc(s.description) : ''}</div>
          </div>
          ${badge}
        </div>
        <table class="info-table">
          <tr><td>令牌</td><td>${s.tokenSet ? '<span class="badge badge-green">已设置</span>' : '<span class="badge badge-red">未设置</span>'}</td></tr>
          <tr><td>模式</td><td>${s.readOnly ? '<span class="badge badge-gray">只读</span>' : '<span class="badge badge-green">可写</span>'}</td></tr>
          <tr><td>请求</td><td class="font-mono">${s.requests || 0} 次 · 失败 <span class="${s.failures ? 'text-red' : ''}">${s.failures || 0}</span> · 平均 ${fmtUs(s.avgUs)}</td></tr>
        </table>
        <div class="ds-conn">${esc(s.connection)}</div>
        <div class="ds-actions">
          ${canPerm('sources') ? `
          <button class="btn btn-primary btn-sm" onclick="copyProjectConn('${esc(s.name)}')">复制项目连接串</button>
          <button class="btn btn-secondary btn-sm" onclick="editSource('${esc(s.name)}')">编辑</button>
          <button class="btn btn-ghost btn-sm" onclick="toggleReadOnly('${esc(s.name)}', ${!s.readOnly})">${s.readOnly ? '解除只读' : '设为只读'}</button>
          <button class="btn btn-ghost btn-sm text-red" onclick="removeSource('${esc(s.name)}')">删除</button>`
          : '<span class="text-muted" style="font-size:11px">只读视图 —— 需要「数据源管理」权限才能编辑、取令牌或复制连接串</span>'}
        </div>
      </div>`;
    }
    html += '</div>';
  }
  html += `</div>`;

  const port = (status.server || {}).port || 5599;
  let host = (status.server || {}).host || '127.0.0.1';
  if (host === '0.0.0.0' || host === '::') host = '127.0.0.1';
  html += `<div class="card glass-card">
    <h2>📌 项目侧连接串怎么用</h2>
    <table class="info-table">
      <tr>
        <td>① 项目里写这一行</td>
        <td class="font-mono">Server=http://${esc(host)}:${esc(port)};Database=&lt;数据源名&gt;;Password=&lt;该数据源的令牌&gt;;provider=network</td>
      </tr>
      <tr>
        <td>&lt;数据源名&gt;</td>
        <td>卡片上的<b>名称</b>，例如 <span class="font-mono">demo</span> —— 它对应连接串里的 <span class="font-mono">Database=</span></td>
      </tr>
      <tr>
        <td>&lt;该数据源的令牌&gt;</td>
        <td>该数据源<b>专属</b>的令牌（每个数据源不同）—— 点卡片上「复制项目连接串」会自动把名称与令牌拼好</td>
      </tr>
      <tr>
        <td>代理机地址</td>
        <td>项目与代理同机用 <span class="font-mono">127.0.0.1</span>；跨机换成代理机的内网 IP，并在「设置」里打开「允许远程访问」</td>
      </tr>
      <tr>
        <td>② 项目代码里</td>
        <td class="font-mono">Dal::open(connection, model) / 任何 Pek.RCode 客户端都行 —— 不需要真实数据库账号密码</td>
      </tr>
    </table>
    <div class="form-desc mt-12">真实连接串以 <span class="font-mono">enc:v1:</span>（AES-256-GCM）加密后只保存在本机 <span class="font-mono">Config/Proxy.toml</span>（密钥 <span class="font-mono">Config/.key</span>）；项目侧只能拿到令牌，即使令牌泄露也拿不到数据库账号。</div>
  </div>`;

  $('panel-overview').innerHTML = html;
}

/* ── 数据源：增删改 ─────────────────────────────────── */

function findSource(name) {
  return (status && status.sources ? status.sources : []).find(s => s.name === name);
}

/* 各数据库类型的默认连接串模板（与 Pek.RPanlServer 同一套口径；套用后仍可继续改） */
const DB_TEMPLATES = {
  sqlite:     'Data Source=Data/demo.db;Provider=SQLite',
  mysql:      'Server=127.0.0.1;Port=3306;Database=demo;Uid=root;Pwd=password;Provider=MySql;SslMode=None',
  postgresql: 'Server=127.0.0.1;Port=5432;Database=demo;Uid=postgres;Pwd=password;Provider=PostgreSQL',
  sqlserver:  'Server=127.0.0.1;Port=1433;Database=demo;Uid=sa;Pwd=password;Provider=SqlServer;Encrypt=false',
  oracle:     'Server=127.0.0.1;Port=1521;ServiceName=xepdb1;Uid=system;Pwd=password;Provider=Oracle',
  dameng:     'Server=127.0.0.1;Port=5236;Database=SYSDBA;Uid=SYSDBA;Pwd=password;Provider=DaMeng',
  clickhouse: 'Server=127.0.0.1;Port=9000;Database=demo;Uid=default;Pwd=password;Provider=ClickHouse',
  tdengine:   'Server=127.0.0.1;Port=6041;Database=iot;Uid=root;Pwd=taosdata;Provider=TDengine',
  mongodb:    'Server=127.0.0.1;Port=27017;Database=demo;Uid=admin;Pwd=password;Provider=MongoDB',
  influxdb:   'Server=127.0.0.1;Database=demo;Uid=root;Pwd=password;Provider=InfluxDB',
  hana:       'Server=127.0.0.1;Port=30015;Uid=SYSTEM;Pwd=password;Provider=HANA',
  firebird:   'Server=127.0.0.1;Port=3050;Database=Data/demo.fdb;Uid=SYSDBA;Pwd=masterkey;Provider=Firebird',
  duckdb:     'Data Source=Data/demo.duckdb;Provider=DuckDB',
  db2:        'Server=127.0.0.1;Port=50000;Database=demo;Uid=db2admin;Pwd=password;Provider=DB2',
  access:     'Data Source=Data/demo.mdb;Provider=Access',
  vastbase:   'Server=127.0.0.1;Port=5432;Database=demo;Uid=root;Pwd=password;Provider=VastBase',
};
const DB_KIND_LABELS = [
  ['sqlite', 'SQLite（内嵌文件）'],
  ['mysql', 'MySQL / MariaDB'],
  ['postgresql', 'PostgreSQL（含瀚高/金仓/海量/VastBase）'],
  ['sqlserver', 'SQL Server'],
  ['oracle', 'Oracle'],
  ['dameng', '达梦 DM'],
  ['clickhouse', 'ClickHouse'],
  ['tdengine', 'TDengine'],
  ['mongodb', 'MongoDB'],
  ['influxdb', 'InfluxDB'],
  ['hana', 'SAP HANA'],
  ['firebird', 'Firebird'],
  ['duckdb', 'DuckDB（内嵌文件）'],
  ['db2', 'DB2'],
  ['access', 'Access'],
  ['vastbase', 'VastBase'],
];
const DB_ALIAS = { maria: 'mysql', mariadb: 'mysql', nova: 'mysql', novadb: 'mysql', postgres: 'postgresql', pgsql: 'postgresql', npgsql: 'postgresql', highgo: 'postgresql', kingbase: 'postgresql', kingbasees: 'postgresql', vastbase: 'postgresql', mssql: 'sqlserver', sqlite3: 'sqlite', dm: 'dameng', td: 'tdengine', mongo: 'mongodb', influx: 'influxdb', sap: 'hana', fb: 'firebird' };

/** 从连接串里的 provider 反推数据库类型（编辑已有数据源时预选下拉）。 */
function dbKindFromConn(s) {
  const m = /provider\s*=\s*([a-z0-9_.]+)/i.exec(s || '');
  if (!m) return '';
  const p = m[1].toLowerCase();
  const k = DB_ALIAS[p] || p;
  return DB_TEMPLATES[k] ? k : '';
}

/** 选中类型后套用该类型的默认连接串（仅填输入框，不保存）。 */
function applyDbTemplate() {
  const sel = $('f-kind');
  const kind = sel.value;
  const tpl = DB_TEMPLATES[kind];
  if (!tpl) return;
  const cur = ($('f-conn').value || '').trim();
  const label = (sel.options[sel.selectedIndex] || {}).text || kind;
  if (cur && !Object.values(DB_TEMPLATES).includes(cur)
    && !window.confirm('把连接串替换为【' + label + '】的默认模板？\n\n' + tpl)) {
    sel.value = dbKindFromConn(cur);
    return;
  }
  $('f-conn').value = tpl;
  const hint = $('f-kind-hint');
  if (hint) hint.textContent = '已套用【' + label + '】模板：改成实际的地址/账号密码即可';
}

async function editSource(name) {
  const s = name ? findSource(name) : null;
  const title = s ? '编辑数据源：' + s.name : '新增数据源';
  // 编辑时把当前连接串回填到输入框（走单独的完整串接口；列表里仍是脱敏值）
  let current = '';
  if (s) {
    try {
      const got = unwrap(await api('/api/datasource/get', { method: 'POST', body: JSON.stringify({ name }) }));
      current = got.connection || '';
    } catch (e) { toast(e.message, 'error'); }
  }
  const kind = current ? dbKindFromConn(current) : (s ? '' : 'sqlite');
  const kindOptions = DB_KIND_LABELS
    .map(([v, t]) => `<option value="${v}" ${kind === v ? 'selected' : ''}>${t}</option>`)
    .join('');
  const body = `
    <div class="form-group">
      <label>名称（项目侧 Database=）</label>
      <input type="text" id="f-name" value="${esc(s ? s.name : '')}" ${s ? 'disabled' : ''}
             placeholder="例如 demo / biz / report">
      <div class="form-desc">${s ? '名称是项目侧连接串里的 Database，创建后不可改' : '只用字母数字与 _-，区分业务库'}</div>
    </div>
    <div class="form-group">
      <label>数据库类型（选中即套用该类型的默认连接串）</label>
      <select id="f-kind" onchange="applyDbTemplate()">
        <option value="">— 不套用模板（手工填写）—</option>
        ${kindOptions}
      </select>
      <div class="form-desc" id="f-kind-hint">与 Pek.RPanlServer 同一套模板：选类型 → 自动填好连接串 → 只改地址/账号密码</div>
    </div>
    <div class="form-group">
      <label>连接串（保存时自动加密为 enc:v1:；也可直接粘贴已有密文）</label>
      <textarea id="f-conn" rows="6" spellcheck="false" autocomplete="off"
        placeholder="Server=127.0.0.1;Port=3306;Database=demo;Uid=root;Pwd=xxx;Provider=MySql">${esc(current || (s ? '' : DB_TEMPLATES.sqlite))}</textarea>
      <div class="form-desc">${s ? '已回填当前连接串（已解密为真实值，可直接修改）；清空则保持原值，保存后自动加密' : '选好类型后模板会自动填入，按实际情况改地址与账号密码；保存时自动加密'}</div>
    </div>
    <div class="form-group">
      <label>令牌（项目侧 Password=）</label>
      <div style="display:flex;gap:8px;align-items:center">
        <input type="text" id="f-token" value="" placeholder="${s ? '（留空 = 不改）' : '（留空 = 自动生成）'}" style="flex:1">
        ${s ? `<button type="button" class="btn btn-secondary btn-sm" onclick="copyToken('${esc(s.name)}')">复制当前令牌</button>` : ''}
      </div>
      <div class="form-desc">令牌只授权访问本数据源，泄露也不会暴露真实数据库账号</div>
    </div>
    <div class="form-group">
      <label>备注</label>
      <input type="text" id="f-desc" value="${esc(s ? s.description : '')}" placeholder="用途说明（可选）">
    </div>
    <div class="form-group" style="display:flex;align-items:center;gap:10px;margin-bottom:0">
      <label style="margin:0">只读模式（拒绝一切写操作）</label>
      <label class="toggle" style="margin-left:auto">
        <input type="checkbox" id="f-ro" ${!s || s.readOnly ? 'checked' : ''}><span class="track"><span class="thumb"></span></span>
      </label>
    </div>`;
  showModal(title, body, '保存', async () => {
    const payload = {
      name: ($('f-name').value || '').trim(),
      connection: ($('f-conn').value || '').trim(),
      token: ($('f-token').value || '').trim(),
      description: ($('f-desc').value || '').trim(),
      readOnly: $('f-ro').checked,
    };
    try {
      unwrap(await api('/api/datasource', { method: 'POST', body: JSON.stringify(payload) }));
      toast('已保存', 'success');
      loadStatus();
      if (panelFromHash() === 'audit') loadAudit();
    } catch (e) { toast(e.message, 'error'); }
  });
}

function removeSource(name) {
  showModal('删除数据源', `确定删除 <b>${esc(name)}</b> 吗？<br>删除后项目侧用该名称的连接串立即失效。`, '删除', async () => {
    try {
      unwrap(await api('/api/datasource/delete', { method: 'POST', body: JSON.stringify({ name }) }));
      toast('已删除', 'success');
      loadStatus();
    } catch (e) { toast(e.message, 'error'); }
  });
}

async function toggleReadOnly(name, readOnly) {
  try {
    unwrap(await api('/api/readonly', { method: 'POST', body: JSON.stringify({ db: name, readOnly }) }));
    toast(readOnly ? '已设为只读' : '已解除只读（允许写操作）', readOnly ? 'success' : 'info');
    loadStatus();
  } catch (e) { toast(e.message, 'error'); }
}

/* ── 复制项目侧连接串（自动带上数据源名与该数据源令牌） ────── */

async function copyText(text) {
  try { await navigator.clipboard.writeText(text); return true; } catch (e) { /* 回退 */ }
  try {
    const ta = document.createElement('textarea');
    ta.value = text;
    ta.style.position = 'fixed';
    ta.style.opacity = '0';
    document.body.appendChild(ta);
    ta.select();
    document.execCommand('copy');
    ta.remove();
    return true;
  } catch (e) { return false; }
}

async function copyToken(name) {
  try {
    const got = unwrap(await api('/api/datasource/token', { method: 'POST', body: JSON.stringify({ name }) }));
    if (await copyText(got.token)) toast('令牌已复制：' + got.token, 'success');
    else showModal('令牌（请手动复制）', `<div class="font-mono" style="word-break:break-all">${esc(got.token)}</div>`, '知道了', null);
  } catch (e) { toast(e.message || '获取令牌失败', 'error'); }
}

async function copyProjectConn(name) {
  try {
    const got = unwrap(await api('/api/datasource/token', { method: 'POST', body: JSON.stringify({ name }) }));
    let host = (status && status.server && status.server.host) || '127.0.0.1';
    if (host === '0.0.0.0' || host === '::') host = '127.0.0.1';
    const port = (status && status.server && status.server.port) || 5599;
    const conn = `Server=http://${host}:${port};Database=${got.name};Password=${got.token};provider=network`;
    if (await copyText(conn)) {
      toast('已复制：' + conn, 'success');
    } else {
      showModal('项目侧连接串（请手动复制）', `<div class="font-mono" style="word-break:break-all">${esc(conn)}</div>`, '知道了', null);
    }
  } catch (e) { toast(e.message || '获取令牌失败', 'error'); }
}

/* ── 字段安全（表管理 / 列策略）───────────────────────── */

const FIELD_ACTION_LABEL = { allow: '允许', mask: '掩码', deny: '拒绝' };

async function loadFields(silent) {
  try {
    fields = unwrap(await api('/api/fields'));
    if (!status) {
      try { status = unwrap(await api('/api/status')); } catch (e) { /* 下拉空缺即可 */ }
    }
  } catch (e) {
    if (!silent) toast(e.message, 'error');
    return;
  }
  renderFields();
}

function renderFields() {
  const d = fields || { mode: 'enforce', rules: [], unknown: {} };
  const enforce = d.mode === 'enforce';
  const rules = d.rules || [];
  const unknownMap = d.unknown || {};
  // 数据源名大小写不敏感匹配
  const unknownByLower = {};
  Object.keys(unknownMap).forEach(k => { unknownByLower[k.toLowerCase()] = unknownMap[k]; });
  const sourceNames = ((status && status.sources) || []).map(s => s.name);
  // 按数据源组织：已配置的 + 规则里出现过的（数据源可能已删）+ 有未登记字段的
  const dbs = [...new Set([...sourceNames, ...rules.map(r => r.db), ...Object.keys(unknownMap)])];

  let html = `
  <div class="card glass-card">
    <h2>🛡 字段安全（表管理）
      <span style="margin-left:auto;display:flex;gap:8px">
        <button class="btn btn-primary btn-sm" onclick="editFieldRule(null)">+ 新增规则</button>
        <button class="btn btn-secondary btn-sm" onclick="loadFields()">刷新</button>
      </span>
    </h2>
    <div class="form-desc">
      <b>字段清单来自实际请求</b>：项目查询返回过哪些列，就出现在对应数据源的「未登记字段」里（已登记的不再列出）。
      <b>未登记列默认不返回</b>；<b>拒绝</b> 的列连 <span class="font-mono">SELECT *</span> 也拿不到，<b>掩码</b> 保留列、值变 <span class="font-mono">***</span>。
      新接入：先跑一次业务查询 → 在该数据源下点「一键允许未登记列」→ 再把敏感列改成拒绝/掩码。
    </div>
    <div class="flex-between" style="margin:14px 0">
      <span class="badge ${enforce ? 'badge-red' : 'badge-gray'}">当前模式：${enforce ? '强制 enforce（未登记列不返回）' : '学习 audit（只记录）'}</span>
      <button class="btn btn-secondary btn-sm" onclick="toggleFieldMode(${enforce ? 'false' : 'true'})">
        ${enforce ? '切回学习模式' : '切换到强制模式'}
      </button>
    </div>
  </div>`;

  if (!dbs.length) {
    html += `<div class="card glass-card"><div class="empty-state"><div class="empty-icon">🛡</div><div>还没有数据源 —— 先在「概览」添加数据源</div></div></div>`;
  }

  for (const db of dbs) {
    const lower = (db || '').toLowerCase();
    const dbRules = rules.filter(r => (r.db || '').toLowerCase() === lower);
    const cols = unknownByLower[lower] || {};
    const dbUnknown = Object.keys(cols).map(column => ({ column, count: cols[column] }));
    const available = sourceNames.some(n => n.toLowerCase() === lower);
    html += `<div class="card glass-card">
      <h2>🗄 ${esc(db)}
        ${available ? '' : '<span class="badge badge-red">数据源已删除</span>'}
        <span class="badge badge-gray">${dbRules.length} 条规则</span>
        <span style="margin-left:auto;display:flex;gap:8px">
          <button class="btn btn-primary btn-sm" onclick="editFieldRule(null,'${esc(db)}')">+ 规则</button>
          ${dbUnknown.length ? `<button class="btn btn-secondary btn-sm" onclick="bulkUnknown('${esc(db)}','allow')">一键允许未登记列（${dbUnknown.length}）</button>` : ''}
        </span>
      </h2>`;

    if (dbUnknown.length) {
      html += `<div class="form-desc">请求返回过、但还没登记的列：</div>
      <table class="svc-table"><thead><tr><th>列名</th><th>出现次数</th><th style="width:240px">快速登记</th></tr></thead><tbody>`;
      for (const u of dbUnknown) {
        html += `<tr>
          <td class="font-mono">${esc(u.column)}</td>
          <td>${u.count}</td>
          <td><div class="svc-actions">
            <button class="btn btn-ghost btn-sm" onclick="quickFieldRule('${esc(db)}','${esc(u.column)}','allow')">允许</button>
            <button class="btn btn-ghost btn-sm text-red" onclick="quickFieldRule('${esc(db)}','${esc(u.column)}','deny')">拒绝</button>
            <button class="btn btn-ghost btn-sm" onclick="quickFieldRule('${esc(db)}','${esc(u.column)}','mask')">掩码</button>
          </div></td></tr>`;
      }
      html += `</tbody></table>`;
    }

    if (dbRules.length) {
      html += `<table class="svc-table" style="margin-top:10px">
        <thead><tr><th>列名</th><th>动作</th><th>备注</th><th style="width:150px">操作</th></tr></thead><tbody>`;
      for (const r of dbRules) {
        const cls = r.action === 'allow' ? 'badge-green' : (r.action === 'mask' ? 'badge-gray' : 'badge-red');
        html += `<tr>
          <td class="font-mono">${esc(r.column)}</td>
          <td><span class="badge ${cls}">${FIELD_ACTION_LABEL[r.action] || esc(r.action)}</span></td>
          <td class="text-muted">${esc(r.note)}</td>
          <td><div class="svc-actions">
            <button class="btn btn-ghost btn-sm" onclick="editFieldRule('${esc(r.db)}','${esc(r.column)}')">编辑</button>
            <button class="btn btn-ghost btn-sm text-red" onclick="removeFieldRule('${esc(r.db)}','${esc(r.column)}')">删除</button>
          </div></td></tr>`;
      }
      html += `</tbody></table>`;
    }

    if (!dbRules.length && !dbUnknown.length) {
      html += `<div class="empty-state"><div class="empty-icon">🔍</div><div>该数据源还没有字段策略 —— 项目跑一次查询后，这里会列出返回过的列</div></div>`;
    }
    html += `</div>`;
  }

  $('panel-fields').innerHTML = html;
}

/* 该数据源可选的列名候选：请求返回过的 + 已登记的 */
function fieldCandidates(db) {
  const out = new Set();
  const lower = (db || '').toLowerCase();
  const map = (fields && fields.unknown) || {};
  Object.keys(map).forEach(k => {
    if (k.toLowerCase() === lower) Object.keys(map[k] || {}).forEach(c => out.add(c));
  });
  ((fields && fields.rules) || []).forEach(r => {
    if ((r.db || '').toLowerCase() === lower) out.add(r.column);
  });
  return [...out].sort();
}

function refreshColumnList() {
  const list = $('f-column-list');
  if (!list) return;
  const db = $('f-db') ? $('f-db').value : '';
  list.innerHTML = fieldCandidates(db).map(c => `<option value="${esc(c)}"></option>`).join('');
}

function editFieldRule(db, column) {
  const rules = (fields && fields.rules) || [];
  const cur = rules.find(r => r.db === db && r.column === column)
    || { db: db || '', column: column || '', action: 'deny', note: '' };
  const names = ((status && status.sources) || []).map(s => s.name);
  const opts = names.map(n => `<option value="${esc(n)}"${n === cur.db ? ' selected' : ''}>${esc(n)}</option>`).join('')
    || '<option value="">（先添加数据源）</option>';
  showModal(db ? '编辑列策略' : '新增列策略', `
    <label>数据源</label>
    <select id="f-db" style="margin-bottom:10px" onchange="refreshColumnList()">${opts}</select>
    <label>列名（可从请求返回过的列里选，也支持手填）</label>
    <input type="text" id="f-column" list="f-column-list" value="${esc(cur.column)}" placeholder="例如 password / mobile" style="margin-bottom:10px">
    <datalist id="f-column-list"></datalist>
    <label>动作</label>
    <select id="f-action" style="margin-bottom:10px">
      <option value="deny"${cur.action === 'deny' ? ' selected' : ''}>拒绝（永不返回，连 SELECT * 也拿不到）</option>
      <option value="mask"${cur.action === 'mask' ? ' selected' : ''}>掩码（列保留，值显示 *** ）</option>
      <option value="allow"${cur.action === 'allow' ? ' selected' : ''}>允许（显式放行）</option>
    </select>
    <label>备注</label>
    <input type="text" id="f-note" value="${esc(cur.note)}" placeholder="用途说明，可空">
  `, '保存', async () => {
    const body = {
      db: $('f-db').value.trim(),
      column: $('f-column').value.trim(),
      action: $('f-action').value,
      note: $('f-note').value.trim(),
    };
    if (!body.db || !body.column) { toast('数据源与列名不能为空', 'error'); return; }
    try {
      unwrap(await api('/api/fields', { method: 'POST', body: JSON.stringify(body) }));
      toast('已保存', 'success');
      await loadFields(true);
    } catch (e) { toast(e.message, 'error'); }
  });
  refreshColumnList();
}

async function quickFieldRule(db, column, action) {
  try {
    unwrap(await api('/api/fields', { method: 'POST', body: JSON.stringify({ db, column, action, note: '' }) }));
    toast(action === 'deny' ? `已登记为拒绝：${column}` : `已登记为允许：${column}`, 'success');
    await loadFields(true);
  } catch (e) { toast(e.message, 'error'); }
}

async function removeFieldRule(db, column) {
  if (!confirm(`删除规则 ${db}.${column}？`)) return;
  try {
    unwrap(await api('/api/fields/delete', { method: 'POST', body: JSON.stringify({ db, column }) }));
    toast('已删除', 'success');
    await loadFields(true);
  } catch (e) { toast(e.message, 'error'); }
}

async function toggleFieldMode(toEnforce) {
  try {
    unwrap(await api('/api/fields/mode', { method: 'POST', body: JSON.stringify({ mode: toEnforce ? 'enforce' : 'audit' }) }));
    toast(toEnforce ? '已切到强制模式：未登记列不再返回' : '已切回学习模式（只记审计）', 'success');
    await loadFields(true);
  } catch (e) { toast(e.message, 'error'); }
}

/* 按数据源批量登记（columns 省略 = 该库已观测到的未登记列） */
async function bulkUnknown(db, action) {
  try {
    const r = unwrap(await api('/api/fields/bulk', {
      method: 'POST',
      body: JSON.stringify({ db, action, note: '批量登记' }),
    }));
    toast(`${db}：已登记 ${r.count} 个列`, 'success');
    await loadFields(true);
  } catch (e) { toast(e.message, 'error'); }
}

/* ── 用户（仅内置管理员可见）───────────────────────────── */

async function loadUsers(silent) {
  try {
    usersData = unwrap(await api('/api/users'));
    if (!status) {
      try { status = unwrap(await api('/api/status')); } catch (e) { /* 忽略 */ }
    }
  } catch (e) {
    if (!silent) toast(e.message, 'error');
    return;
  }
  renderUsers();
}

function permName(key) {
  const list = (usersData && usersData.permissions) || [];
  const hit = list.find(p => p.key === key || p.name === key);
  return hit ? (hit.name || key) : key;
}

function renderUsers() {
  const d = usersData || { users: [], permissions: [] };
  const rows = [];
  if (d.builtinAdmin) rows.push(Object.assign({ builtin: true }, d.builtinAdmin));
  (d.users || []).forEach(u => rows.push(u));

  let html = `<div class="card glass-card">
    <h2>👥 面板用户
      <span style="margin-left:auto;display:flex;gap:8px">
        <button class="btn btn-primary btn-sm" onclick="editUser(null)">+ 新增用户</button>
        <button class="btn btn-secondary btn-sm" onclick="loadUsers()">刷新</button>
      </span>
    </h2>
    <div class="form-desc">
      内置管理员来自 <span class="font-mono">Config/Proxy.toml</span>（在下方 <span class="font-mono">admin</span> 那一行点「编辑」改用户名/密码；恒有全部权限、不可删除）；
      下表用户存放在 <span class="font-mono">Data/admin.db</span>，按勾选的页签授权；密码只要求非空（不强制复杂度）。
    </div>
    <table class="svc-table" style="margin-top:12px">
      <thead><tr><th>用户名</th><th>页签权限</th><th>状态</th><th>备注</th><th style="width:190px">操作</th></tr></thead><tbody>`;
  for (const u of rows) {
    const tags = (u.permissionNames && u.permissionNames.length
      ? u.permissionNames
      : (u.permissions || []).map(permName))
      .map(n => `<span class="badge badge-gray" style="margin:1px 3px">${esc(n)}</span>`).join('');
    html += `<tr>
      <td class="font-mono">${esc(u.userName)}${u.builtin ? ' <span class="badge badge-green">内置</span>' : ''}</td>
      <td>${tags || '<span class="text-muted">（无）</span>'}</td>
      <td>${u.enabled ? '<span class="badge badge-green">启用</span>' : '<span class="badge badge-red">停用</span>'}</td>
      <td class="text-muted">${esc(u.remark)}</td>
      <td>${u.builtin
        ? `<button class="btn btn-ghost btn-sm" onclick="editUser('${esc(u.userName)}')">编辑</button>`
        : `<div class="svc-actions">
             <button class="btn btn-ghost btn-sm" onclick="editUser('${esc(u.userName)}')">编辑</button>
             <button class="btn btn-ghost btn-sm text-red" onclick="removeUser('${esc(u.userName)}')">删除</button>
           </div>`}</td>
    </tr>`;
  }
  html += `</tbody></table></div>`;
  $('panel-users').innerHTML = html;
}

function editUser(name) {
  // 内置管理员：只改用户名/密码（权限恒全、不可删除；凭据在 Config/Proxy.toml）
  if (name && usersData && usersData.builtinAdmin && name === usersData.builtinAdmin.userName) {
    const cur = usersData.builtinAdmin;
    showModal('内置管理员（配置文件）', `
      <label>用户名</label>
      <input type="text" id="u-name" value="${esc(cur.userName)}" style="margin-bottom:10px">
      <label>新密码（留空 = 不改；不强制复杂度）</label>
      <input type="text" id="u-pass" placeholder="留空表示保持现有密码" style="margin-bottom:10px">
      <div class="form-desc">存在 <span class="font-mono">Config/Proxy.toml</span>，恒有全部权限、不可删除。</div>
    `, '保存', async () => {
      const body = { builtin: true, name: $('u-name').value.trim(), password: $('u-pass').value };
      if (!body.name) { toast('用户名不能为空', 'error'); return; }
      try {
        unwrap(await api('/api/user', { method: 'POST', body: JSON.stringify(body) }));
        toast('已保存', 'success');
        await loadMe();
        await loadUsers(true);
      } catch (e) { toast(e.message, 'error'); }
    });
    return;
  }
  const perms = (usersData && usersData.permissions) || [];
  const cur = name ? ((usersData && usersData.users) || []).find(u => u.userName === name) : null;
  if (name && !cur) { toast('用户不存在（先刷新）', 'error'); return; }
  const curPerms = cur ? (cur.permissions || []) : [];
  const checks = perms.map(p => `
    <label style="display:flex;align-items:center;gap:8px;margin:5px 0;font-size:13px">
      <input type="checkbox" class="f-perm" value="${esc(p.key)}"${curPerms.includes(p.key) ? ' checked' : ''}>
      <span>${esc(p.name)}</span>
    </label>`).join('');
  showModal(cur ? `编辑用户：${esc(name)}` : '新增用户', `
    <label>用户名</label>
    <input type="text" id="u-name" value="${esc(cur ? cur.userName : '')}" placeholder="例如 ops" style="margin-bottom:10px">
    <label>密码${cur ? '（留空 = 不改）' : ''}：不强制复杂度</label>
    <input type="text" id="u-pass" placeholder="${cur ? '留空表示保持现有密码' : '输入密码'}" style="margin-bottom:10px">
    <label style="display:flex;align-items:center;gap:8px;margin-bottom:10px">
      <input type="checkbox" id="u-enabled"${!cur || cur.enabled ? ' checked' : ''}><span>允许登录</span>
    </label>
    <label>页签权限</label>
    <div style="margin-bottom:10px">${checks || '<span class="text-muted">（无可授权限）</span>'}</div>
    <label>备注</label>
    <input type="text" id="u-remark" value="${esc(cur ? cur.remark : '')}" placeholder="可空">
  `, '保存', async () => {
    const body = {
      name: $('u-name').value.trim(),
      originalName: cur ? cur.userName : '',
      password: $('u-pass').value,
      enabled: $('u-enabled').checked,
      permissions: [...document.querySelectorAll('.f-perm')].filter(c => c.checked).map(c => c.value),
      remark: $('u-remark').value.trim(),
    };
    if (!body.name) { toast('用户名不能为空', 'error'); return; }
    if (!cur && !body.password.trim()) { toast('新用户必须设置密码', 'error'); return; }
    try {
      unwrap(await api('/api/user', { method: 'POST', body: JSON.stringify(body) }));
      toast('已保存', 'success');
      await loadUsers(true);
    } catch (e) { toast(e.message, 'error'); }
  });
}

async function removeUser(name) {
  if (!confirm(`删除用户 ${name}？`)) return;
  try {
    unwrap(await api('/api/user/delete', { method: 'POST', body: JSON.stringify({ name }) }));
    toast('已删除', 'success');
    await loadUsers(true);
  } catch (e) { toast(e.message, 'error'); }
}

/* ── 审计 ───────────────────────────────────────────── */

async function loadAudit() {
  const n = 100;
  let rows = [];
  try {
    rows = unwrap(await api('/api/audit?n=' + n)) || [];
  } catch (e) { toast(e.message, 'error'); return; }

  let html = `<div class="card glass-card">
    <h2>📋 SQL 审计
      <span style="margin-left:auto;display:flex;gap:8px;align-items:center">
        <select id="audit-n" onchange="loadAudit()" style="margin:0">
          <option value="50">最近 50</option>
          <option value="100" selected>最近 100</option>
          <option value="300">最近 300</option>
          <option value="500">最近 500</option>
        </select>
        <button class="btn btn-secondary btn-sm" onclick="loadAudit()">刷新</button>
      </span>
    </h2>`;
  if (!rows.length) {
    html += `<div class="empty-state"><div class="empty-icon">📋</div><div>暂无记录（项目侧调用 /Db 后在此显示）</div></div>`;
  } else {
    html += `<table class="svc-table"><thead><tr>
      <th>时间</th><th>数据源</th><th>操作</th><th>结果</th><th>耗时</th><th>行数</th><th>令牌</th><th>SQL（备注 / 错误）</th>
      </tr></thead><tbody>`;
    for (const r of rows) {
      const sqlCell = r.sql
        ? `<div class="audit-sql" title="${esc(r.sql)}">${esc(r.sql)}</div>`
        : '<div class="audit-sql text-muted">—</div>';
      const noteCell = r.note ? `<div class="audit-note">备注：${esc(r.note)}</div>` : '';
      const errCell = r.error ? `<div class="audit-err">错误：${esc(r.error)}</div>` : '';
      html += `<tr>
        <td class="font-mono" style="white-space:nowrap">${esc(r.at)}</td>
        <td>${esc(r.db)}</td>
        <td>${esc(r.op)}</td>
        <td>${r.ok ? '<span class="badge badge-green">成功</span>' : '<span class="badge badge-red">失败</span>'}</td>
        <td class="font-mono" style="white-space:nowrap">${fmtUs(r.micros)}</td>
        <td class="font-mono">${r.rows < 0 ? '—' : esc(r.rows)}</td>
        <td class="font-mono text-muted">${esc(r.token || '—')}</td>
        <td class="font-mono" style="word-break:break-all;max-width:520px">${sqlCell}${noteCell}${errCell}</td>
      </tr>`;
    }
    html += '</tbody></table>';
  }
  html += '</div>';
  $('panel-audit').innerHTML = html;
  const sel = $('audit-n');
  if (sel && n) sel.value = String(Math.min(500, Math.max(50, n)));
}

/* ── 设置 ───────────────────────────────────────────── */

function loadSettings() {
  if (!status) { loadStatus().then(loadSettings); return; }
  const s = status.server || {};
  const levels = [
    ['None', 'None（不鉴权，仅适合完全内网）'],
    ['LocalOnly', 'LocalOnly（本机免登录，远程需登录）'],
    ['Full', 'Full（所有访问都需登录）'],
  ];
  const html = `
  <div class="card glass-card">
    <h2>⚙ 服务端设置</h2>
    <table class="config-table">
      <tr>
        <td>监听端口</td>
        <td><input type="number" id="s-port" value="${esc(s.port)}" min="1" max="65535" style="width:140px"></td>
        <td>当前 ${esc(s.host)}:${esc(s.port)}；端口修改后需重启服务生效</td>
      </tr>
      <tr>
        <td>允许远程访问</td>
        <td>
          <label class="toggle-wrap">
            <span class="toggle">
              <input type="checkbox" id="s-remote" ${s.allowRemote ? 'checked' : ''} onchange="markDirty(this)"><span class="track"><span class="thumb"></span></span>
            </span>
            <span class="state" id="s-remote-state">${s.allowRemote ? '已开启' : '已关闭'}</span>
          </label>
        </td>
        <td>关闭时只接受本机回环请求。开启后需<b>重启服务</b>才生效，且需自行在系统防火墙放行端口：<br>
          Windows：<span class="font-mono">netsh advfirewall firewall add rule name="PekRDbProxy" dir=in action=allow protocol=TCP localport=${esc(s.port)}</span><br>
          Linux：<span class="font-mono">firewall-cmd --add-port=${esc(s.port)}/tcp --permanent &amp;&amp; firewall-cmd --reload</span>（ufw：<span class="font-mono">ufw allow ${esc(s.port)}/tcp</span>）<br>
          云服务器还需在安全组放行同一端口。</td>
      </tr>
      <tr>
        <td>面板仅限本机</td>
        <td>
          <label class="toggle-wrap">
            <span class="toggle">
              <input type="checkbox" id="s-panel-local" ${s.panelLocalOnly !== false ? 'checked' : ''} onchange="markDirty(this)"><span class="track"><span class="thumb"></span></span>
            </span>
            <span class="state" id="s-panel-local-state">${s.panelLocalOnly !== false ? '已开启' : '已关闭'}</span>
          </label>
        </td>
        <td>开启（推荐）时别的机器<b>打不开面板</b>，只能用令牌访问 <span class="font-mono">/Db/*</span> 查数据——开发人员看不到真实连接串、也改不了字段策略。<br>
          关掉后远程也能开面板，务必把鉴权级别设为 <span class="font-mono">Full</span> 并改掉默认密码。</td>
      </tr>
      <tr>
        <td>面板鉴权级别</td>
        <td>
          <select id="s-level" style="width:100%;max-width:420px">
            ${levels.map(([v, t]) => `<option value="${v}" ${s.webAuthLevel === v ? 'selected' : ''}>${t}</option>`).join('')}
          </select>
        </td>
        <td>对齐 Pek.RAgent：None / LocalOnly / Full</td>
      </tr>
      <tr>
        <td>内置管理员</td>
        <td colspan="2">用户名/密码在「👥 用户」页的 <span class="font-mono">${esc(s.webUserName)}</span> 那一行点「编辑」修改
          （存在 <span class="font-mono">Config/Proxy.toml</span>，恒有全部权限、不可删除）。</td>
      </tr>
      <tr>
        <td></td>
        <td colspan="2"><button class="btn btn-primary btn-sm" onclick="saveSettings()">保存设置</button></td>
      </tr>
    </table>
  </div>

  <div class="card glass-card">
    <h2>⬆ 在线升级</h2>
    <div class="form-inline" style="margin-bottom:8px">
      <input type="file" id="up-file" accept=".exe,application/octet-stream" style="max-width:320px">
      <button class="btn btn-primary btn-sm" onclick="uploadUpgrade()">上传并升级</button>
    </div>
    <div class="form-desc">选择新的程序文件（Windows 为 <span class="font-mono">pek-rdbproxy.exe</span>，Linux 为无扩展名二进制）上传；写入 <span class="font-mono">Update/</span> 后约 10 秒自动替换并重启，期间面板短暂断开属正常。也可以直接把文件拷进 <span class="font-mono">Update/</span> 目录。</div>
  </div>

  <div class="card glass-card">
    <h2>🧩 驱动组件</h2>
    <div class="form-desc" style="margin-bottom:10px">
      本程序<b>内置</b> SQLite 与 <span class="font-mono" id="drv-builtin">—</span>；其余库类型（Oracle / 达梦 / MongoDB / HANA / Firebird / ODBC …）
      首次打开数据源时自动从组件源下载 <span class="font-mono">dbserver-&lt;类型&gt;</span> 驱动包并拉起宿主进程，
      <b>驱动组件升级不必重编/重发本程序</b>。
    </div>
    <div id="drv-body"><div class="form-desc">正在读取组件源…</div></div>
  </div>

  <div class="card glass-card">
    <h2>ℹ 运行信息</h2>
    <table class="info-table">
      <tr><td>程序</td><td>Pek.RDbProxy v${esc(status.version || '0.1.0')}</td></tr>
      <tr><td>运行时长</td><td>${fmtUptime(status.uptimeSecs)}</td></tr>
      <tr><td>数据源</td><td>${(status.sources || []).length} 个（${(status.sources || []).map(x => esc(x.name)).join('、') || '—'}）</td></tr>
      <tr><td>配置文件</td><td class="font-mono">Config/Proxy.toml（连接串已加密；密钥 Config/.key）</td></tr>
      <tr><td>升级方式</td><td>设置页「⬆ 在线升级」上传，或把程序放入 Update/（10 秒后自动替换重启）</td></tr>
    </table>
  </div>`;
  $('panel-settings').innerHTML = html;
  loadDrivers();
}

/* ── 驱动组件（按需下载） ───────────────────────────── */

let driversData = null;

async function loadDrivers(silent) {
  const box = $('drv-body');
  if (!box) return;
  if (!silent) box.innerHTML = '<div class="form-desc">正在读取组件源…</div>';
  try {
    driversData = unwrap(await api('/api/drivers'));
    const builtin = $('drv-builtin');
    if (builtin) builtin.textContent = (driversData.builtinProviders || []).join(' / ') || '—';
    renderDrivers();
  } catch (e) {
    box.innerHTML = `<div class="err">读取驱动组件失败：${esc(e.message)}</div>`;
  }
}

function renderDrivers() {
  const box = $('drv-body');
  const d = driversData;
  if (!box || !d) return;
  const c = d.config || {};
  const stateText = d.offline
    ? '已关闭联网（仅本机缓存）'
    : (d.online ? '组件源可达' : '组件源不可达');
  const stateCls = d.offline ? 'off' : (d.online ? 'on' : 'bad');
  const rows = (d.items || []).map(it => {
    const badge = it.builtin ? '<span class="tag tag-ok">内置</span>' : '<span class="tag">组件</span>';
    const local = it.local ? esc(it.local) : '<span class="muted">未安装</span>';
    const latest = it.latest ? esc(it.latest) : '<span class="muted">—</span>';
    const op = it.upgradable
      ? `<button class="btn btn-primary btn-sm" onclick="installDriver('${esc(it.id)}')">${it.local ? '升级' : '安装'}</button>`
      : (it.local ? '<span class="muted">已就绪</span>' : '<span class="muted">—</span>');
    return `<tr>
      <td class="font-mono">${esc(it.id)}</td>
      <td>${esc(it.name)}</td>
      <td>${badge}</td>
      <td>${local}</td>
      <td>${latest}</td>
      <td>${it.running || 0}</td>
      <td>${op}</td>
    </tr>`;
  }).join('');
  const hosts = (d.hosts || []).map(h =>
    `<li><span class="font-mono">${esc(h.component)} ${esc(h.version)}</span> · ${esc(h.addr)} · pid ${esc(h.pid)}</li>`
  ).join('');

  box.innerHTML = `
  <div class="form-desc" style="margin-bottom:8px">
    组件源：<span class="font-mono">${esc(d.store || '—')}</span>
    <span class="drv-state ${stateCls}">${stateText}</span>
    · 缓存：<span class="font-mono">${esc(d.cacheDir || '—')}</span>
    · 平台：<span class="font-mono">${esc(d.target || '—')}</span>
    · 组件优先：<b>${d.componentFirst ? '是' : '否'}</b>
    <button class="btn btn-ghost btn-sm" onclick="loadDrivers()">刷新</button>
  </div>
  ${d.error ? `<div class="err" style="margin-bottom:8px">组件源访问失败：${esc(d.error)}</div>` : ''}
  ${rows ? `<table class="config-table" style="margin-bottom:10px">
    <tr><th>组件</th><th>名称</th><th>类型</th><th>本地版本</th><th>最新版本</th><th>运行中</th><th>操作</th></tr>
    ${rows}
  </table>` : '<div class="form-desc" style="margin-bottom:10px">没有可显示的组件（组件源不可达且本机无缓存）。</div>'}
  ${hosts ? `<div class="form-desc" style="margin-bottom:10px">运行中的驱动宿主：<ul style="margin:6px 0 0 18px">${hosts}</ul></div>` : ''}
  <table class="config-table">
    <tr>
      <td>组件源地址</td>
      <td><input id="drv-url" value="${esc(c.url || '')}" placeholder="留空 = 平台默认地址；off = 仅本机缓存" style="width:100%;max-width:420px"></td>
      <td>驱动包来源（Pek.RPanlServer 组件库）；离线环境填 off 并把驱动包解压到缓存目录</td>
    </tr>
    <tr>
      <td>公钥（Ed25519）</td>
      <td><input id="drv-pubkey" value="${esc(c.pubKey || '')}" placeholder="留空 = 不验签" style="width:100%;max-width:420px"></td>
      <td>填写后强制验签，并允许内网 http 组件源</td>
    </tr>
    <tr>
      <td>组件源根证书</td>
      <td><input id="drv-ca" value="${esc(c.caFile || '')}" placeholder="留空 = 用公共根；自签/内网 CA 填 PEM 路径" style="width:100%;max-width:420px"></td>
      <td>相对路径按程序基目录解析</td>
    </tr>
    <tr>
      <td>缓存目录</td>
      <td><input id="drv-cache" value="${esc(c.cacheDir || '')}" placeholder="留空 = Data/Drivers" style="width:100%;max-width:420px"></td>
      <td>驱动包解压后的存放位置</td>
    </tr>
    <tr>
      <td>组件优先</td>
      <td>
        <label class="toggle-wrap">
          <span class="toggle">
            <input type="checkbox" id="drv-first" ${c.componentFirst ? 'checked' : ''}><span class="track"><span class="thumb"></span></span>
          </span>
        </label>
      </td>
      <td>开启后连内置驱动也强制走组件（驱动完全随组件源单独升级，首次使用会联网下载）；关闭时内置优先，已缓存的组件仍会优先使用</td>
    </tr>
    <tr>
      <td></td>
      <td colspan="2"><button class="btn btn-primary btn-sm" onclick="saveDriverConfig()">保存并重载驱动</button></td>
    </tr>
  </table>`;
}

async function installDriver(id) {
  try {
    const r = unwrap(await api('/api/drivers/install', { method: 'POST', body: JSON.stringify({ id }) }));
    toast(`${id} 已安装：${r.version}（重开数据源或重启服务后生效）`, 'success');
    loadDrivers(true);
  } catch (e) { toast(e.message, 'error'); }
}

async function saveDriverConfig() {
  const payload = {
    url: $('drv-url').value.trim(),
    pubKey: $('drv-pubkey').value.trim(),
    caFile: $('drv-ca').value.trim(),
    cacheDir: $('drv-cache').value.trim(),
    componentFirst: $('drv-first').checked,
  };
  try {
    unwrap(await api('/api/drivers/config', { method: 'POST', body: JSON.stringify(payload) }));
    toast('驱动组件配置已保存，数据源已按新配置重开', 'success');
    loadDrivers(true);
  } catch (e) { toast(e.message, 'error'); }
}

/* 开关变化：更新文字并提示需要保存（避免"勾了没反应"的误解） */
function markDirty(el) {
  const map = { 's-remote': 's-remote-state', 's-panel-local': 's-panel-local-state' };
  const state = $(map[el.id]);
  if (state) state.textContent = el.checked ? '已开启' : '已关闭';
  toast('已修改，点下方「保存设置」生效', 'info');
}

async function saveSettings() {
  const payload = {
    port: parseInt($('s-port').value, 10) || undefined,
    allowRemote: $('s-remote').checked,
    panelLocalOnly: $('s-panel-local').checked,
    authLevel: $('s-level').value,
  };
  try {
    const r = unwrap(await api('/api/server', { method: 'POST', body: JSON.stringify(payload) }));
    if (r.restartRequired) toast('已保存；端口/远程访问变更需重启服务后生效', 'info');
    else toast('已保存', 'success');
    loadStatus();
  } catch (e) { toast(e.message, 'error'); }
}

/* ── 在线升级 ─────────────────────────────────────── */

async function uploadUpgrade() {
  const inp = $('up-file');
  if (!inp || !inp.files || !inp.files.length) { toast('请先选择程序文件', 'error'); return; }
  const f = inp.files[0];
  if (!window.confirm(`上传并升级到 ${f.name}（${Math.round(f.size / 1024)} KB）？升级期间面板会短暂断开。`)) return;
  try {
    const headers = {};
    if (token) headers['Authorization'] = 'Bearer ' + token;
    const r = await fetch('/api/upgrade', { method: 'POST', headers, body: f });
    const el = r.headers.get('X-Elapsed-Ms'); if (el) setSrvElapsed(el);
    const j = await r.json();
    if (!r.ok || !j.ok) throw new Error((j && j.message) || '上传失败');
    toast((j.data && j.data.note) || '已上传，即将重启', 'success');
  } catch (e) { toast(e.message, 'error'); }
}

/* ── 主题 ───────────────────────────────────────────── */

function toggleTheme() {
  const cur = document.documentElement.classList.contains('light') ? 'light' : 'dark';
  const next = cur === 'light' ? 'dark' : 'light';
  document.documentElement.classList.toggle('light', next === 'light');
  try { localStorage.setItem('rdbproxy_theme', next); } catch (e) {}
}

(function init() {
  // 局部样式：本面板表格列较多，避免徽章被挤成两行（沿用 app.css 的配色与尺寸，仅补 nowrap）
  const st = document.createElement('style');
  st.textContent = `
.svc-table .badge{white-space:nowrap}
.ds-grid{display:grid;grid-template-columns:repeat(auto-fill,minmax(370px,1fr));gap:14px}
.ds-card{padding:16px 18px;margin-bottom:0;display:flex;flex-direction:column}
.ds-name{font-size:15px;font-weight:700;letter-spacing:-0.01em}
.ds-card .info-table td{padding:5px 0;font-size:12px;border-bottom:1px solid var(--border-subtle);width:auto}
.ds-card .info-table td:first-child{width:56px;padding-right:10px;font-size:11px;font-weight:600;color:var(--text-tertiary)}
.ds-card .info-table tr:last-child td{border-bottom:none}
.ds-conn{margin-top:10px;padding:7px 10px;border-radius:var(--radius-sm);background:var(--surface-2);
  font-family:var(--font-mono);font-size:11px;color:var(--text-muted);word-break:break-all;line-height:1.5}
.ds-actions{display:flex;gap:8px;margin-top:12px;flex-wrap:wrap}
#f-conn{width:100%;box-sizing:border-box;min-height:150px;padding:10px 12px;border:1px solid var(--border-default);
  border-radius:var(--radius-md);background:var(--surface-2);color:var(--text-primary);font-family:var(--font-mono);
  font-size:12px;line-height:1.7;outline:none;resize:vertical;word-break:break-all}
#f-conn:focus{border-color:var(--brand-500);box-shadow:0 0 0 3px rgba(91,91,255,.25)}
#f-kind{width:100%;box-sizing:border-box}
#modal-body .form-desc{word-break:break-all;white-space:normal;line-height:1.6}
`;
  document.head.appendChild(st);

  try {
    // 默认浅色（与 Pek 系面板一致）；显式选过深色才移除 light
    if (localStorage.getItem('rdbproxy_theme') === 'dark') {
      document.documentElement.classList.remove('light');
    }
  } catch (e) {}
  token = readToken();
  $('lpass').addEventListener('keydown', (e) => { if (e.key === 'Enter') doLogin(); });
  $('luser').addEventListener('keydown', (e) => { if (e.key === 'Enter') $('lpass').focus(); });
  if (token) {
    // 有令牌：先验一次（LocalOnly 下即使无令牌也放行，status 会成功）
    enterApp();
  } else {
    // 本地回环且鉴权 LocalOnly 时免登录：探测 status
    fetch('/api/status').then(r => r.json()).then(j => {
      if (j && j.ok) { enterApp(); }
    }).catch(() => {});
  }
})();

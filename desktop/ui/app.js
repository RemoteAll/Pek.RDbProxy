/* Pek.RDbProxy 连接控制台 —— 纯静态前端（无打包器）。
   逻辑全在 Rust（core crate），这里只做输入、渲染与错误提示。 */

const invoke = window.__TAURI__.core.invoke;

const CONN_KEY = 'rdbproxy_tool_conn';
let connected = false;

const $ = (id) => document.getElementById(id);

/* ── 小工具 ───────────────────────────────────── */

function toast(message, kind = 'info') {
  const el = $('toast');
  el.textContent = message;
  el.className = 'toast show' + (kind === 'error' ? ' error' : '');
  clearTimeout(toast._timer);
  toast._timer = setTimeout(() => {
    el.className = 'toast' + (kind === 'error' ? ' error' : '');
  }, kind === 'error' ? 5200 : 2600);
}

function esc(value) {
  return String(value ?? '')
    .replaceAll('&', '&amp;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;')
    .replaceAll('"', '&quot;');
}

function setState(text, kind) {
  const el = $('conn-state');
  el.textContent = text;
  el.className = 'state' + (kind ? ' ' + kind : '');
  const dot = $('conn-dot');
  dot.className = 'dot' + (kind === 'ok' ? ' ok' : kind === 'bad' ? ' bad' : '');
}

function connString() {
  return $('conn').value.trim();
}

function requireConn() {
  if (!connString()) {
    toast('请先填写连接串', 'error');
    throw new Error('缺少连接串');
  }
  return connString();
}

async function call(command, args) {
  return invoke(command, args);
}

function busy(button, on) {
  if (!button) return;
  button.disabled = on;
}

/* 统一处理命令错误（Rust 侧返回 Err(String) 会变成 rejected promise） */
async function guard(button, fn) {
  busy(button, true);
  try {
    return await fn();
  } catch (e) {
    const message = typeof e === 'string' ? e : (e && e.message) || String(e);
    toast(message, 'error');
    return null;
  } finally {
    busy(button, false);
  }
}

/* ── 标签页 ───────────────────────────────────── */

document.querySelectorAll('.tab').forEach((tab) => {
  tab.addEventListener('click', () => {
    document.querySelectorAll('.tab').forEach((t) => t.classList.toggle('active', t === tab));
    const name = tab.dataset.tab;
    document.querySelectorAll('.panel').forEach((panel) => {
      panel.classList.toggle('active', panel.id === 'tab-' + name);
    });
  });
});

/* ── 连接 ─────────────────────────────────────── */

async function doLogin() {
  const conn = requireConn();
  const info = await guard($('btn-login'), () => call('tool_login', { conn }));
  if (!info) {
    connected = false;
    setState('连接失败', 'bad');
    return;
  }
  connected = true;
  const parts = [info.dbType || '已连接'];
  if (info.version) parts.push(info.version);
  setState(`${parts.join(' · ')}（${info.elapsedMs} ms）`, 'ok');
  try { localStorage.setItem(CONN_KEY, conn); } catch (e) { /* 隐私模式忽略 */ }
  toast('连接正常', 'info');
}

$('btn-login').addEventListener('click', doLogin);
$('conn').addEventListener('keydown', (event) => {
  if (event.key === 'Enter') doLogin();
});

/* ── 结果渲染 ─────────────────────────────────── */

function cellText(value) {
  if (value === null || value === undefined) return { text: 'NULL', isNull: true };
  if (typeof value === 'object') return { text: JSON.stringify(value), isNull: false };
  return { text: String(value), isNull: false };
}

function renderRows(columns, rows, title, meta, note) {
  $('result-title').textContent = title || '结果';
  $('result-meta').textContent = meta || '';
  $('result-error').classList.add('hidden');
  const noteEl = $('result-note');
  if (note) {
    noteEl.textContent = note;
    noteEl.classList.remove('hidden');
  } else {
    noteEl.classList.add('hidden');
  }
  if (!rows || rows.length === 0) {
    $('result-body').innerHTML = '<div class="empty">查询成功，没有数据行</div>';
    return;
  }
  const head = columns.map((c) => `<th>${esc(c)}</th>`).join('');
  const body = rows
    .map((row) => {
      const cells = row
        .map((value) => {
          const { text, isNull } = cellText(value);
          return `<td${isNull ? ' class="null"' : ''}>${esc(text)}</td>`;
        })
        .join('');
      return `<tr>${cells}</tr>`;
    })
    .join('');
  $('result-body').innerHTML = `<table class="result"><thead><tr>${head}</tr></thead><tbody>${body}</tbody></table>`;
}

function showError(message) {
  $('result-title').textContent = '失败';
  $('result-meta').textContent = '';
  $('result-note').classList.add('hidden');
  const box = $('result-error');
  box.textContent = message;
  box.classList.remove('hidden');
  $('result-body').innerHTML = '';
}

/* ── 控制台动作 ───────────────────────────────── */

async function runQuery() {
  const conn = requireConn();
  const sql = $('sql').value.trim();
  if (!sql) return toast('请填写 SQL', 'error');
  const result = await guard($('btn-query'), () => call('tool_query', { conn, sql }));
  if (!result) return showError('查询失败（见下方提示）');
  renderRows(result.columns, result.rows, '查询结果',
    `${result.rowCount} 行 · ${result.columns.length} 列 · ${result.elapsedMs} ms`, result.note);
}

async function runExecute() {
  const conn = requireConn();
  const sql = $('sql').value.trim();
  if (!sql) return toast('请填写 SQL', 'error');
  const result = await guard($('btn-execute'), () => call('tool_execute', { conn, sql }));
  if (!result) return showError('执行失败（只读数据源会被拒绝，这是预期行为）');
  $('result-title').textContent = '执行结果';
  $('result-meta').textContent = `${result.elapsedMs} ms`;
  $('result-error').classList.add('hidden');
  $('result-note').classList.add('hidden');
  $('result-body').innerHTML = `<div class="empty">影响 ${esc(result.affected)} 行</div>`;
}

async function runTables() {
  const conn = requireConn();
  const result = await guard($('btn-tables'), () => call('tool_tables', { conn }));
  if (!result) return showError('读取表结构失败');
  renderRows(['Name'], result.tables.map((t) => [t.name ?? t.Name ?? JSON.stringify(t)]),
    '表结构', `${result.count} 张表 · ${result.elapsedMs} ms`);
}

async function runCount() {
  const conn = requireConn();
  const table = $('count-table').value.trim();
  if (!table) return toast('请填写表名', 'error');
  const result = await guard($('btn-count'), () => call('tool_count', { conn, table }));
  if (!result) return showError('读取记录数失败');
  renderRows([result.table], [[result.count]], '记录数', `${result.elapsedMs} ms`);
}

$('btn-query').addEventListener('click', runQuery);
$('btn-execute').addEventListener('click', runExecute);
$('btn-tables').addEventListener('click', runTables);
$('btn-count').addEventListener('click', runCount);

/* ── 自检 ─────────────────────────────────────── */

async function runSelfCheck() {
  const conn = requireConn();
  const options = {
    sql: $('check-sql').value.trim(),
    table: $('check-table').value.trim(),
    writeSql: $('check-write').value.trim(),
    checkWrongToken: $('check-wrong').checked,
  };
  const report = await guard($('btn-check'), () => call('tool_selfcheck', { conn, options }));
  if (!report) {
    $('check-list').innerHTML = '<div class="empty">自检失败（见下方提示）</div>';
    $('check-meta').textContent = '';
    return;
  }
  $('check-meta').textContent =
    `通过 ${report.passed} · 失败 ${report.failed} · 跳过 ${report.skipped} · 总 ${report.totalMs} ms`;
  const markOf = { pass: '✔', fail: '✘', skip: '○' };
  $('check-list').innerHTML = report.items
    .map((item) => `
      <div class="check ${esc(item.status)}">
        <div class="mark">${markOf[item.status] || '·'}</div>
        <div class="name">${esc(item.name)}</div>
        <div class="ms">${esc(item.elapsedMs)} ms</div>
        <div class="detail">${esc(item.detail)}</div>
      </div>`)
    .join('');
  if (report.failed === 0) {
    setState('自检通过', 'ok');
    toast('全部通过', 'info');
  } else {
    setState(`自检有 ${report.failed} 项失败`, 'bad');
    toast(`有 ${report.failed} 项失败`, 'error');
  }
}

$('btn-check').addEventListener('click', runSelfCheck);

/* ── 关于 ─────────────────────────────────────── */

$('btn-forget').addEventListener('click', () => {
  try { localStorage.removeItem(CONN_KEY); } catch (e) { /* ignore */ }
  $('conn').value = '';
  setState('未连接');
  toast('已清除本机保存的连接串');
});

async function loadInfo() {
  try {
    const info = await call('tool_info');
    $('app-ver').textContent = 'v' + info.version;
    $('info-version').textContent = info.version;
    $('info-ua').textContent = info.userAgent;
    $('info-protocol').textContent = info.protocol;
  } catch (e) {
    $('app-ver').textContent = 'v?';
  }
}

/* ── 启动 ─────────────────────────────────────── */

(function boot() {
  try {
    const saved = localStorage.getItem(CONN_KEY);
    if (saved) $('conn').value = saved;
  } catch (e) { /* ignore */ }
  loadInfo();
  $('conn').focus();
})();

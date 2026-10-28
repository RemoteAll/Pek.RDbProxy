/* Pek.RDbProxy 连接控制台 —— 纯静态前端（无打包器）。
   业务逻辑在 Rust（core crate）；这里负责工作区渲染、对话框与结果展示。
   令牌从不下发到前端：保存的连接只用 id，执行时由 Rust 侧解密。 */

const invoke = window.__TAURI__.core.invoke;

const LS = {
  current: 'rdbtool_current',   // 上次选中的连接
  draft: 'rdbtool_draft_',      // 每个连接一份 SQL 草稿
};

let ws = { connections: [], snippets: [], history: [], historyLimit: 200, dataDir: '' };
let info = null;
let current = null;            // { kind:'saved', id } | { kind:'adhoc', conn } | null
let search = '';
let histSearch = '';
let lastResult = null;         // 最近一次查询结果（复制 CSV/JSON 用）
let lastCheck = null;          // 最近一次自检报告（复制用）

const $ = (id) => document.getElementById(id);

/* ── 小工具 ───────────────────────────────────── */

function toast(message, kind = 'info') {
  const el = $('toast');
  el.textContent = message;
  el.className = 'toast show' + (kind === 'error' ? ' error' : '');
  clearTimeout(toast._timer);
  toast._timer = setTimeout(() => {
    el.className = 'toast' + (kind === 'error' ? ' error' : '');
  }, kind === 'error' ? 5200 : 2400);
}

function esc(value) {
  return String(value ?? '')
    .replaceAll('&', '&amp;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;')
    .replaceAll('"', '&quot;')
    .replaceAll("'", '&#39;');
}

function fmtTime(ms) {
  if (!ms) return '—';
  const d = new Date(Number(ms));
  const pad = (n) => String(n).padStart(2, '0');
  const today = new Date();
  const sameDay = d.toDateString() === today.toDateString();
  const time = `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
  return sameDay ? time : `${d.getMonth() + 1}-${pad(d.getDate())} ${time}`;
}

async function call(command, args) {
  return invoke(command, args || {});
}

/** 统一错误提示（Rust 侧 Err(String) → rejected promise）。 */
async function guard(button, fn) {
  if (button) button.disabled = true;
  try {
    return await fn();
  } catch (e) {
    toast(typeof e === 'string' ? e : (e && e.message) || String(e), 'error');
    return null;
  } finally {
    if (button) button.disabled = false;
  }
}

async function copyText(text, label = '已复制') {
  try {
    await navigator.clipboard.writeText(text);
    toast(label);
    return;
  } catch (e) { /* 回退 */ }
  const ta = document.createElement('textarea');
  ta.value = text;
  ta.style.position = 'fixed';
  ta.style.opacity = '0';
  document.body.appendChild(ta);
  ta.select();
  try {
    document.execCommand('copy');
    toast(label);
  } catch (e) {
    toast('复制失败：' + e, 'error');
  }
  ta.remove();
}

/* ── 通用对话框 ───────────────────────────────── */

let dialogOk = null;

function openDialog({ title, body, okText = '确定', ok, onMount }) {
  $('dlg-title').textContent = title;
  $('dlg-body').innerHTML = body;
  $('dlg-ok').textContent = okText;
  dialogOk = ok || null;
  $('overlay').classList.remove('hidden');
  if (onMount) onMount();
  const first = $('dlg-body').querySelector('input, textarea, select');
  if (first) first.focus();
}

function closeDialog() {
  $('overlay').classList.add('hidden');
  dialogOk = null;
}

$('dlg-cancel').addEventListener('click', closeDialog);
$('dlg-ok').addEventListener('click', async () => {
  if (!dialogOk) return closeDialog();
  const ok = dialogOk;
  const button = $('dlg-ok');
  button.disabled = true;
  try {
    const keep = await ok();
    if (keep !== false) closeDialog();
  } catch (e) {
    toast(typeof e === 'string' ? e : (e && e.message) || String(e), 'error');
  } finally {
    button.disabled = false;
  }
});

/* ── 工作区 ───────────────────────────────────── */

function applyWs(view) {
  if (!view) return;
  ws = view;
  renderAll();
}

async function reloadWs() {
  const view = await guard(null, () => call('ws_load'));
  if (view) applyWs(view);
}

function currentId() {
  return current && current.kind === 'saved' ? current.id : null;
}

function currentConn() {
  return current && current.kind === 'adhoc' ? current.conn : null;
}

function connectionOf(id) {
  return ws.connections.find((c) => c.id === id) || null;
}

function draftKey() {
  return current ? (current.kind === 'saved' ? current.id : 'adhoc') : 'none';
}

function saveDraft() {
  try { localStorage.setItem(LS.draft + draftKey(), $('sql').value); } catch (e) { /* ignore */ }
}

function restoreDraft() {
  try {
    const saved = localStorage.getItem(LS.draft + draftKey());
    if (saved !== null && saved !== '') $('sql').value = saved;
  } catch (e) { /* ignore */ }
}

/* ── 渲染：左栏 ───────────────────────────────── */

function renderSide() {
  $('conn-count').textContent = ws.connections.length ? String(ws.connections.length) : '';
  const kw = search.trim().toLowerCase();
  const conns = ws.connections.filter((c) =>
    !kw ||
    c.name.toLowerCase().includes(kw) ||
    (c.project || '').toLowerCase().includes(kw) ||
    c.db.toLowerCase().includes(kw) ||
    (c.note || '').toLowerCase().includes(kw));
  const box = $('conn-list');
  if (!conns.length) {
    box.innerHTML = `<div class="empty">${ws.connections.length ? '没有匹配的连接' : '还没有保存连接，点上方 ＋ 新建'}</div>`;
  } else {
    const groups = new Map();
    for (const c of conns) {
      const key = c.project || '（未分组）';
      if (!groups.has(key)) groups.set(key, []);
      groups.get(key).push(c);
    }
    let html = '';
    for (const [group, items] of [...groups.entries()].sort((a, b) => a[0].localeCompare(b[0], 'zh'))) {
      html += `<div class="group-label">${esc(group)}</div>`;
      for (const c of items) {
        const active = currentId() === c.id ? ' active' : '';
        const dot = c.lastOk === true ? 'ok' : c.lastOk === false ? 'bad' : '';
        html += `<div class="item${active}" data-kind="conn" data-id="${esc(c.id)}" title="${esc(c.connection)}">
          <span class="badge ${dot}"></span>
          <span class="name">${esc(c.name)}</span>
          <span class="sub">${esc(c.db)}</span>
          <span class="acts">
            <button data-act="edit-conn" data-id="${esc(c.id)}">改</button>
            <button data-act="del-conn" data-id="${esc(c.id)}">删</button>
          </span>
        </div>`;
      }
    }
    box.innerHTML = html;
  }

  const snips = ws.snippets.filter((s) =>
    !kw ||
    s.name.toLowerCase().includes(kw) ||
    (s.project || '').toLowerCase().includes(kw) ||
    (s.note || '').toLowerCase().includes(kw) ||
    s.sql.toLowerCase().includes(kw));
  const sbox = $('snip-list');
  if (!snips.length) {
    sbox.innerHTML = `<div class="empty">${ws.snippets.length ? '没有匹配的片段' : '把常用 SQL 存成片段，点上方 ＋'}</div>`;
  } else {
    sbox.innerHTML = snips
      .map((s) => `<div class="item" data-kind="snip" data-id="${esc(s.id)}" title="${esc(s.sql)}">
        <span class="name">${esc(s.name)}</span>
        <span class="sub">${s.usedCount ? '用 ' + s.usedCount + ' 次' : esc(s.project || '')}</span>
        <span class="acts">
          <button data-act="run-snip" data-id="${esc(s.id)}">跑</button>
          <button data-act="edit-snip" data-id="${esc(s.id)}">改</button>
          <button data-act="del-snip" data-id="${esc(s.id)}">删</button>
        </span>
      </div>`)
      .join('');
  }
}

/* ── 渲染：顶栏当前连接 ────────────────────────── */

function renderConnBar() {
  const dot = $('conn-dot');
  const name = $('cur-name');
  const meta = $('cur-meta');
  const health = $('cur-health');
  dot.className = 'dot';
  if (current && current.kind === 'adhoc') {
    name.textContent = '临时连接';
    meta.textContent = current.conn.replace(/Password=([^;]*)/i, 'Password=••••');
    health.textContent = '';
    health.className = 'health none';
    return;
  }
  const c = currentId() ? connectionOf(currentId()) : null;
  if (!c) {
    name.textContent = '未选择连接';
    meta.textContent = '';
    health.textContent = '';
    health.className = 'health none';
    return;
  }
  name.textContent = c.name;
  meta.textContent = `${c.project ? c.project + ' · ' : ''}${c.server} · ${c.db}`;
  if (c.lastOk === true) {
    health.textContent = `上次连接成功 ${fmtTime(c.lastTryMs)}`;
    health.className = 'health ok';
    dot.className = 'dot ok';
  } else if (c.lastOk === false) {
    health.textContent = `上次连接失败 ${fmtTime(c.lastTryMs)}`;
    health.className = 'health bad';
    dot.className = 'dot bad';
  } else {
    health.textContent = '还没连过';
    health.className = 'health none';
  }
}

/* ── 渲染：历史 ───────────────────────────────── */

function renderHistory() {
  $('hist-count').textContent = ws.history.length ? String(ws.history.length) : '';
  $('hist-hint').textContent = `（最多保留 ${ws.historyLimit} 条，同连接同 SQL 只留最新）`;
  const kw = histSearch.trim().toLowerCase();
  const list = ws.history.filter((h) =>
    !kw ||
    h.sql.toLowerCase().includes(kw) ||
    (h.connectionName || '').toLowerCase().includes(kw));
  const box = $('hist-list');
  if (!list.length) {
    box.innerHTML = `<div class="empty">${ws.history.length ? '没有匹配的记录' : '还没有执行记录（用保存的连接执行 SQL 后自动记录）'}</div>`;
    return;
  }
  box.innerHTML = list
    .map((h) => `<div class="hist">
      <div>
        <div class="sql">${esc(h.sql)}</div>
        <div class="meta">${fmtTime(h.atMs)} · ${esc(h.connectionName || '（连接已删除）')} · ${h.ok ? '成功' : '失败'} · ${h.rows < 0 ? '—' : h.rows} 行 · ${h.micros} ms</div>
      </div>
      <div class="acts">
        <span class="badge ${h.ok ? 'ok' : 'bad'}">${h.ok ? '成功' : '失败'}</span>
        <button class="btn small" data-act="use-hist" data-id="${esc(h.id)}">填入</button>
        <button class="btn small" data-act="save-hist" data-id="${esc(h.id)}">存为片段</button>
        <button class="btn small ghost" data-act="del-hist" data-id="${esc(h.id)}">删</button>
      </div>
    </div>`)
    .join('');
}

function renderAll() {
  renderSide();
  renderConnBar();
  renderHistory();
}

/* ── 选择 / 登录 ──────────────────────────────── */

function selectConnection(id) {
  if (currentId() === id) return;
  saveDraft();
  current = { kind: 'saved', id };
  try { localStorage.setItem(LS.current, JSON.stringify(current)); } catch (e) { /* ignore */ }
  renderSide();
  renderConnBar();
  restoreDraft();
}

function useAdhoc() {
  openDialog({
    title: '临时连接（不保存）',
    body: `<label class="field"><span>连接串</span>
      <textarea id="f-adhoc" style="min-height:90px">Server=http://127.0.0.1:5599;Database=demo;Password=;provider=network</textarea></label>
      <div class="hint">只在本机内存中使用，不写入工作区；临时连接不记执行历史。</div>`,
    okText: '使用',
    ok: () => {
      const conn = $('f-adhoc').value.trim();
      if (!conn) throw new Error('连接串不能为空');
      saveDraft();
      current = { kind: 'adhoc', conn };
      try { localStorage.removeItem(LS.current); } catch (e) { /* ignore */ }
      renderSide();
      renderConnBar();
      restoreDraft();
      toast('已切换到临时连接');
    },
  });
}

async function doLogin() {
  if (!current) return toast('请先选择或新建一个连接', 'error');
  const view = await guard($('btn-login'), () =>
    call('tool_login', { connId: currentId(), conn: currentConn() }));
  if (!view) {
    await reloadWs();
    return;
  }
  const text = [view.dbType || '已连接', view.version].filter(Boolean).join(' · ');
  toast(`连接正常：${text}（${view.elapsedMs} ms）`);
  await reloadWs();
}

/* ── 执行 ─────────────────────────────────────── */

function ensureCurrent() {
  if (!current) {
    toast('请先在左侧选择连接', 'error');
    throw new Error('未选择连接');
  }
}

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
  if (note) { noteEl.textContent = note; noteEl.classList.remove('hidden'); }
  else noteEl.classList.add('hidden');
  if (!rows || !rows.length) {
    $('result-body').innerHTML = '<div class="empty">查询成功，没有数据行</div>';
    return;
  }
  const head = (columns || []).map((c) => `<th>${esc(c)}</th>`).join('');
  const body = rows.map((row) => {
    const cells = row.map((value) => {
      const { text, isNull } = cellText(value);
      return `<td${isNull ? ' class="null"' : ''}>${esc(text)}</td>`;
    }).join('');
    return `<tr>${cells}</tr>`;
  }).join('');
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

async function runQuery() {
  ensureCurrent();
  const sql = $('sql').value.trim();
  if (!sql) return toast('请填写 SQL', 'error');
  const view = await guard($('btn-query'), () =>
    call('tool_query', { connId: currentId(), conn: currentConn(), sql }));
  await reloadWs();
  if (!view) { lastResult = null; return showError('查询失败（见上方提示）'); }
  lastResult = view;
  renderRows(view.columns, view.rows, '查询结果',
    `${view.rowCount} 行 · ${view.columns.length} 列 · ${view.elapsedMs} ms`, view.note);
}

async function runExecute() {
  ensureCurrent();
  const sql = $('sql').value.trim();
  if (!sql) return toast('请填写 SQL', 'error');
  if (!window.confirm('这会执行写语句，确认？\n\n' + sql)) return;
  const view = await guard($('btn-execute'), () =>
    call('tool_execute', { connId: currentId(), conn: currentConn(), sql }));
  await reloadWs();
  if (!view) return showError('执行失败（只读数据源会被拒绝，这是预期行为）');
  lastResult = null;
  $('result-title').textContent = '执行结果';
  $('result-meta').textContent = `${view.elapsedMs} ms`;
  $('result-error').classList.add('hidden');
  $('result-note').classList.add('hidden');
  $('result-body').innerHTML = `<div class="empty">影响 ${esc(view.affected)} 行</div>`;
}

async function runTables() {
  ensureCurrent();
  const view = await guard($('btn-tables'), () =>
    call('tool_tables', { connId: currentId(), conn: currentConn() }));
  await reloadWs();
  if (!view) return showError('读取表结构失败');
  lastResult = { columns: ['Name'], rows: view.tables.map((t) => [t.name ?? t.Name ?? JSON.stringify(t)]) };
  renderRows(lastResult.columns, lastResult.rows, '表结构', `${view.count} 张表 · ${view.elapsedMs} ms`);
}

async function runCount() {
  ensureCurrent();
  const table = $('count-table').value.trim();
  if (!table) return toast('请填写表名', 'error');
  const view = await guard($('btn-count'), () =>
    call('tool_count', { connId: currentId(), conn: currentConn(), table }));
  await reloadWs();
  if (!view) return showError('读取记录数失败');
  lastResult = { columns: [view.table], rows: [[view.count]] };
  renderRows(lastResult.columns, lastResult.rows, '记录数', `${view.elapsedMs} ms`);
}

/* ── 连接：增删改 ─────────────────────────────── */

function editConnection(id) {
  const c = connectionOf(id) || {
    name: '', project: '', server: 'http://127.0.0.1:5599', db: '', note: '', tokenSet: false,
  };
  openDialog({
    title: id ? `编辑连接：${c.name}` : '新建连接',
    body: `
      <label class="field"><span>名称 *</span><input id="f-name" value="${esc(c.name)}" placeholder="如：HlktechIoT 开发库"></label>
      <div class="two">
        <label class="field"><span>项目（分组）</span><input id="f-project" value="${esc(c.project)}" placeholder="如：HlktechIoT"></label>
        <label class="field"><span>数据源名 *</span><input id="f-db" value="${esc(c.db)}" placeholder="面板上的数据源名"></label>
      </div>
      <label class="field"><span>代理地址 *</span><input id="f-server" value="${esc(c.server)}" placeholder="http://127.0.0.1:5599"></label>
      <label class="field"><span>令牌 ${id ? '（留空 = 不改）' : '*'}</span>
        <input id="f-token" type="password" placeholder="${c.tokenSet ? '已保存，留空不改' : '数据源令牌'}"></label>
      <label class="field"><span>备注</span><input id="f-note" value="${esc(c.note)}"></label>
      <div class="hint">令牌用本机密钥加密后落盘，界面与导出以外都只显示掩码。</div>`,
    okText: '保存',
    ok: async () => {
      const input = {
        id: id || null,
        name: $('f-name').value,
        project: $('f-project').value,
        server: $('f-server').value,
        db: $('f-db').value,
        token: $('f-token').value,
        note: $('f-note').value,
      };
      const view = await call('ws_save_connection', { input });
      applyWs(view);
      if (!id) {
        const saved = ws.connections.find((x) => x.name === input.name.trim() && x.db === input.db.trim());
        if (saved) selectConnection(saved.id);
      }
      toast('已保存连接');
    },
  });
}

async function deleteConnection(id) {
  const c = connectionOf(id);
  if (!c) return;
  if (!window.confirm(`删除连接「${c.name}」？\n历史记录会保留，片段会自动解绑。`)) return;
  const view = await guard(null, () => call('ws_delete_connection', { id }));
  if (!view) return;
  if (currentId() === id) {
    current = null;
    try { localStorage.removeItem(LS.current); } catch (e) { /* ignore */ }
  }
  applyWs(view);
  toast('已删除连接');
}

/* ── 片段 ─────────────────────────────────────── */

function editSnippet(id, presetSql) {
  const s = ws.snippets.find((x) => x.id === id) || {
    name: '', sql: presetSql || $('sql').value, note: '', project: '', connectionId: currentId() || '',
  };
  const options = ['<option value="">通用（任何连接都能用）</option>']
    .concat(ws.connections.map((c) =>
      `<option value="${esc(c.id)}" ${c.id === s.connectionId ? 'selected' : ''}>${esc(c.project ? c.project + ' / ' : '')}${esc(c.name)}</option>`))
    .join('');
  openDialog({
    title: id ? `编辑片段：${s.name}` : '存为片段',
    body: `
      <label class="field"><span>名称 *</span><input id="f-sname" value="${esc(s.name)}" placeholder="如：查未同步的订单"></label>
      <div class="two">
        <label class="field"><span>项目</span><input id="f-sproject" value="${esc(s.project)}" placeholder="留空 = 通用"></label>
        <label class="field"><span>绑定连接</span><select id="f-sconn">${options}</select></label>
      </div>
      <label class="field"><span>SQL *</span><textarea id="f-ssql" style="min-height:120px">${esc(s.sql)}</textarea></label>
      <label class="field"><span>备注（什么时候用、要注意什么）</span><input id="f-snote" value="${esc(s.note)}"></label>`,
    okText: '保存',
    ok: async () => {
      const input = {
        id: id || null,
        name: $('f-sname').value,
        sql: $('f-ssql').value,
        note: $('f-snote').value,
        project: $('f-sproject').value,
        connectionId: $('f-sconn').value,
      };
      const view = await call('ws_save_snippet', { input });
      applyWs(view);
      toast('已保存片段');
    },
  });
}

async function deleteSnippet(id) {
  const s = ws.snippets.find((x) => x.id === id);
  if (!s) return;
  if (!window.confirm(`删除片段「${s.name}」？`)) return;
  const view = await guard(null, () => call('ws_delete_snippet', { id }));
  if (view) { applyWs(view); toast('已删除片段'); }
}

async function useSnippet(id, run) {
  const s = ws.snippets.find((x) => x.id === id);
  if (!s) return;
  $('sql').value = s.sql;
  saveDraft();
  const view = await guard(null, () => call('ws_touch_snippet', { id }));
  if (view) applyWs(view);
  switchTab('console');
  if (run) await runQuery();
}

/* ── 历史 ─────────────────────────────────────── */

async function deleteHistory(id) {
  const view = await guard(null, () => call('ws_delete_history', { id }));
  if (view) applyWs(view);
}

async function clearHistory() {
  if (!ws.history.length) return;
  if (!window.confirm('清空全部执行历史？此操作不可撤销。')) return;
  const view = await guard(null, () => call('ws_clear_history'));
  if (view) { applyWs(view); toast('已清空历史'); }
}

/* ── 导入 / 导出 ──────────────────────────────── */

async function exportWorkspace() {
  const data = await guard(null, () => call('ws_export'));
  if (!data) return;
  openDialog({
    title: '导出工作区',
    body: `<div class="err">导出内容含<b>明文令牌</b>，请只放在自己可信的位置，不要提交到仓库或发到群里。</div>
      <label class="field"><span>JSON（可直接全选复制，或点下方「保存为文件」）</span>
      <textarea id="f-export" style="min-height:200px" readonly>${esc(data.json)}</textarea></label>
      <div class="row"><button class="btn" id="f-download">保存为文件</button></div>`,
    okText: '复制并关闭',
    ok: async () => { await copyText($('f-export').value, '已复制导出内容'); },
    onMount: () => {
      $('f-download').addEventListener('click', () => downloadText('rdbproxy-workspace.json', $('f-export').value));
    },
  });
}

function downloadText(filename, text) {
  try {
    const blob = new Blob([text], { type: 'application/json' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = filename;
    a.click();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
    toast('已导出到下载目录');
  } catch (e) {
    toast('保存失败：' + e, 'error');
  }
}

function importWorkspace() {
  openDialog({
    title: '导入工作区',
    body: `<div class="hint" style="margin-bottom:8px">按「名称 + 数据源」合并连接（同名覆盖令牌），片段追加。跨机导入会换新片段 id。</div>
      <label class="field"><span>选择文件</span><input type="file" id="f-file" accept=".json,application/json"></label>
      <label class="field"><span>或粘贴 JSON</span><textarea id="f-import" style="min-height:160px" spellcheck="false"></textarea></label>`,
    okText: '导入',
    ok: async () => {
      const text = $('f-import').value.trim();
      if (!text) throw new Error('请选择文件或粘贴 JSON');
      const result = await call('ws_import', { jsonText: text });
      applyWs(result.workspace);
      toast(`导入完成：${result.connections} 个连接、${result.snippets} 个片段`);
    },
    onMount: () => {
      $('f-file').addEventListener('change', (event) => {
        const file = event.target.files && event.target.files[0];
        if (!file) return;
        const reader = new FileReader();
        reader.onload = () => { $('f-import').value = String(reader.result || ''); };
        reader.onerror = () => toast('读取文件失败', 'error');
        reader.readAsText(file, 'utf-8');
      });
    },
  });
}

/* ── 自检 ─────────────────────────────────────── */

async function runSelfCheck() {
  ensureCurrent();
  const options = {
    sql: $('check-sql').value.trim(),
    table: $('check-table').value.trim(),
    writeSql: $('check-write').value.trim(),
    checkWrongToken: $('check-wrong').checked,
  };
  const report = await guard($('btn-check'), () =>
    call('tool_selfcheck', { connId: currentId(), conn: currentConn(), options }));
  await reloadWs();
  if (!report) {
    $('check-list').innerHTML = '<div class="empty">自检失败（见上方提示）</div>';
    $('check-meta').textContent = '';
    lastCheck = null;
    return;
  }
  lastCheck = report;
  $('check-meta').textContent =
    `通过 ${report.passed} · 失败 ${report.failed} · 跳过 ${report.skipped} · 总 ${report.totalMs} ms`;
  const markOf = { pass: '✔', fail: '✘', skip: '○' };
  $('check-list').innerHTML = report.items
    .map((item) => `<div class="check ${esc(item.status)}">
        <div class="mark">${markOf[item.status] || '·'}</div>
        <div class="name">${esc(item.name)}</div>
        <div class="ms">${esc(item.elapsedMs)} ms</div>
        <div class="detail">${esc(item.detail)}</div>
      </div>`)
    .join('');
  toast(report.failed === 0 ? '自检全部通过' : `自检有 ${report.failed} 项失败`,
    report.failed === 0 ? 'info' : 'error');
}

/* ── 复制结果 ─────────────────────────────────── */

function resultToCsv() {
  if (!lastResult) return null;
  const cols = lastResult.columns || [];
  const rows = lastResult.rows || [];
  const cell = (v) => {
    const text = v === null || v === undefined ? '' : (typeof v === 'object' ? JSON.stringify(v) : String(v));
    return /[",\n]/.test(text) ? `"${text.replaceAll('"', '""')}"` : text;
  };
  return [cols.map(cell).join(','), ...rows.map((r) => r.map(cell).join(','))].join('\n');
}

/* ── 标签页 ───────────────────────────────────── */

function switchTab(name) {
  document.querySelectorAll('.tab').forEach((t) => t.classList.toggle('active', t.dataset.tab === name));
  document.querySelectorAll('.panel').forEach((p) => p.classList.toggle('active', p.id === 'tab-' + name));
}

document.querySelectorAll('.tab').forEach((tab) => {
  tab.addEventListener('click', () => switchTab(tab.dataset.tab));
});

/* ── 事件绑定 ─────────────────────────────────── */

$('btn-login').addEventListener('click', doLogin);
$('btn-adhoc').addEventListener('click', useAdhoc);
$('btn-add-conn').addEventListener('click', () => editConnection(null));
$('btn-add-snip').addEventListener('click', () => editSnippet(null));
$('btn-save-snip').addEventListener('click', () => editSnippet(null, $('sql').value));
$('btn-query').addEventListener('click', runQuery);
$('btn-execute').addEventListener('click', runExecute);
$('btn-tables').addEventListener('click', runTables);
$('btn-count').addEventListener('click', runCount);
$('btn-check').addEventListener('click', runSelfCheck);
$('btn-clear-history').addEventListener('click', clearHistory);
$('btn-export').addEventListener('click', exportWorkspace);
$('btn-import').addEventListener('click', importWorkspace);

$('btn-copy-csv').addEventListener('click', () => {
  const csv = resultToCsv();
  if (!csv) return toast('还没有可复制的结果', 'error');
  copyText(csv, '已复制 CSV');
});
$('btn-copy-json').addEventListener('click', () => {
  if (!lastResult) return toast('还没有可复制的结果', 'error');
  copyText(JSON.stringify(lastResult, null, 2), '已复制 JSON');
});
$('btn-copy-check').addEventListener('click', () => {
  if (!lastCheck) return toast('还没有自检结果', 'error');
  const lines = lastCheck.items.map((i) => `${i.status === 'pass' ? '✔' : i.status === 'fail' ? '✘' : '○'} ${i.name}（${i.elapsedMs} ms）${i.detail}`);
  copyText([`目标：${lastCheck.target}`, ...lines,
    `通过 ${lastCheck.passed} / 失败 ${lastCheck.failed} / 跳过 ${lastCheck.skipped}（${lastCheck.totalMs} ms）`].join('\n'),
    '已复制自检报告');
});

$('search').addEventListener('input', (e) => { search = e.target.value; renderSide(); });
$('hist-search').addEventListener('input', (e) => { histSearch = e.target.value; renderHistory(); });
$('sql').addEventListener('input', () => saveDraft());

$('conn-list').addEventListener('click', (event) => {
  const btn = event.target.closest('button[data-act]');
  if (btn) {
    event.stopPropagation();
    const { act, id } = btn.dataset;
    if (act === 'edit-conn') editConnection(id);
    if (act === 'del-conn') deleteConnection(id);
    return;
  }
  const item = event.target.closest('.item[data-kind="conn"]');
  if (item) selectConnection(item.dataset.id);
});
$('conn-list').addEventListener('dblclick', (event) => {
  if (event.target.closest('button')) return;
  const item = event.target.closest('.item[data-kind="conn"]');
  if (item) { selectConnection(item.dataset.id); doLogin(); }
});

$('snip-list').addEventListener('click', (event) => {
  const btn = event.target.closest('button[data-act]');
  if (btn) {
    event.stopPropagation();
    const { act, id } = btn.dataset;
    if (act === 'run-snip') useSnippet(id, true);
    if (act === 'edit-snip') editSnippet(id);
    if (act === 'del-snip') deleteSnippet(id);
    return;
  }
  const item = event.target.closest('.item[data-kind="snip"]');
  if (item) useSnippet(item.dataset.id, false);
});
$('snip-list').addEventListener('dblclick', (event) => {
  if (event.target.closest('button')) return;
  const item = event.target.closest('.item[data-kind="snip"]');
  if (item) useSnippet(item.dataset.id, true);
});

$('hist-list').addEventListener('click', (event) => {
  const btn = event.target.closest('button[data-act]');
  if (!btn) return;
  const { act, id } = btn.dataset;
  const item = ws.history.find((h) => h.id === id);
  if (act === 'use-hist' && item) {
    $('sql').value = item.sql;
    saveDraft();
    switchTab('console');
  }
  if (act === 'save-hist' && item) editSnippet(null, item.sql);
  if (act === 'del-hist') deleteHistory(id);
});

/* 快捷键 */
document.addEventListener('keydown', (event) => {
  const ctrl = event.ctrlKey || event.metaKey;
  if (!ctrl) return;
  if (event.key === 'Enter') {
    event.preventDefault();
    if (event.shiftKey) runExecute(); else runQuery();
    return;
  }
  if (event.key.toLowerCase() === 'k') { event.preventDefault(); $('search').focus(); return; }
  if (event.key.toLowerCase() === 'l') { event.preventDefault(); doLogin(); return; }
  if (event.key.toLowerCase() === 's') { event.preventDefault(); editSnippet(null, $('sql').value); }
});

/* 对话框回车提交（文本域里不拦） */
$('dlg-body').addEventListener('keydown', (event) => {
  if (event.key === 'Enter' && !event.shiftKey && event.target.tagName !== 'TEXTAREA') {
    event.preventDefault();
    $('dlg-ok').click();
  }
});

/* ── 启动 ─────────────────────────────────────── */

async function loadInfo() {
  try {
    info = await call('tool_info');
    $('app-ver').textContent = 'v' + info.version;
    $('info-version').textContent = info.version;
    $('info-ua').textContent = info.userAgent;
    $('info-protocol').textContent = info.protocol;
    $('info-dir').textContent = info.dataDir;
    $('info-hist').textContent = `${info.historyLimit} 条`;
  } catch (e) {
    $('app-ver').textContent = 'v?';
  }
}

(async function boot() {
  await loadInfo();
  await reloadWs();
  try {
    const saved = JSON.parse(localStorage.getItem(LS.current) || 'null');
    if (saved && saved.kind === 'saved' && connectionOf(saved.id)) current = saved;
  } catch (e) { /* ignore */ }
  renderAll();
  restoreDraft();
})();

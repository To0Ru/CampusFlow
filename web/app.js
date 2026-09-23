/* CampusFlow 桌面版前端 —— 原生 JS，无框架，通过 Tauri IPC 和后端通信 */

const T = window.__TAURI__;
const invoke = T?.core?.invoke;
const listen = T?.event?.listen;

const $ = (id) => document.getElementById(id);
const el = {
  ssid: $('ssid'), dot: $('dot'),
  hero: $('hero'), heroTitle: $('heroTitle'), heroSub: $('heroSub'),
  btnFix: $('btnFix'), fixSpin: $('fixSpin'), fixLabel: $('fixLabel'),
  btnRefresh: $('btnRefresh'), btnPortal: $('btnPortal'),
  btnLogout: $('btnLogout'), btnQuit: $('btnQuit'),

  vPortal: $('vPortal'), vOutport: $('vOutport'), vUser: $('vUser'),
  vIp: $('vIp'), vDuration: $('vDuration'), vBalance: $('vBalance'),

  watchToggle: $('watchToggle'), watchInterval: $('watchInterval'),
  checkMode: $('checkMode'), modeHint: $('modeHint'), intervalWrap: $('intervalWrap'),
  wChecks: $('wChecks'), wFixes: $('wFixes'), wLast: $('wLast'),

  log: $('log'), autoScroll: $('autoScroll'), btnClearLog: $('btnClearLog'),

  cfgForm: $('cfgForm'), fUser: $('fUser'), fPass: $('fPass'),
  fChannel: $('fChannel'), fAutostart: $('fAutostart'),
  fAutoExit: $('fAutoExit'), autoExitRow: $('autoExitRow'),
  btnSave: $('btnSave'),
  cfgMsg: $('cfgMsg'), pwdNote: $('pwdNote'), about: $('about'),

  toast: $('toast'), tabs: $('tabs'),
};

let busy = false;
let portalURL = 'http://10.102.250.36';

/* ------------------------------------------------------------------ */
/* 工具                                                                */
/* ------------------------------------------------------------------ */

function toast(msg, kind = '') {
  el.toast.textContent = msg;
  el.toast.className = 'toast show ' + kind;
  clearTimeout(toast._t);
  toast._t = setTimeout(() => { el.toast.className = 'toast ' + kind; }, 3000);
}

function hhmm(ts) {
  if (!ts) return '—';
  const d = new Date(ts);
  return d.toLocaleTimeString('zh-CN', { hour12: false });
}

function fmtDuration(sec) {
  const n = parseInt(sec, 10);
  if (!Number.isFinite(n) || n <= 0) return '—';
  const h = Math.floor(n / 3600);
  const m = Math.floor((n % 3600) / 60);
  const s = n % 60;
  if (h) return `${h} 小时 ${m} 分`;
  if (m) return `${m} 分 ${s} 秒`;
  return `${s} 秒`;
}

function setBusy(on, label) {
  busy = on;
  el.btnFix.disabled = on;
  el.btnRefresh.disabled = on;
  el.btnLogout.disabled = on;
  el.fixSpin.hidden = !on;
  el.fixLabel.textContent = on ? (label || '处理中…') : '一键连接';
  if (on) {
    el.hero.className = 'hero is-busy';
    el.heroTitle.textContent = label || '处理中…';
    el.heroSub.textContent = '正在与认证门户交互，过程见「日志」';
  }
}

/* ------------------------------------------------------------------ */
/* Tab                                                                 */
/* ------------------------------------------------------------------ */

el.tabs.addEventListener('click', (e) => {
  const btn = e.target.closest('.tab');
  if (!btn) return;
  for (const t of el.tabs.querySelectorAll('.tab')) t.classList.toggle('is-on', t === btn);
  for (const p of document.querySelectorAll('.pane')) {
    p.classList.toggle('is-on', p.dataset.pane === btn.dataset.pane);
  }
  if (btn.dataset.pane === 'log') el.log.scrollTop = el.log.scrollHeight;
});

/* ------------------------------------------------------------------ */
/* 状态                                                                */
/* ------------------------------------------------------------------ */

function renderStatus(s) {
  el.ssid.textContent = s.ssid || '未连接 WiFi';
  el.vUser.textContent = s.portal_user || s.username || '—';
  el.vIp.textContent = s.ip || '—';
  el.vOutport.textContent = s.outport || s.channel || '—';
  el.vDuration.textContent = fmtDuration(s.duration);
  el.vBalance.textContent = s.balance != null && s.balance !== '' ? '￥' + s.balance : '—';

  const st = s.portal_state;
  el.vPortal.textContent = st === 'on' ? '在线' : st === 'off' ? '未认证' : '未知';
  el.vPortal.className = 'v ' + (st === 'on' ? 'on' : st === 'off' ? 'off' : '');

  // 自动重连统计
  const w = s.watcher || {};
  el.wChecks.textContent = w.checks ?? 0;
  el.wFixes.textContent = w.fixes ?? 0;
  el.wLast.textContent = w.last_check_ms ? hhmm(w.last_check_ms) : '—';
  el.watchToggle.checked = !!w.running;
  if (w.interval) el.watchInterval.value = w.interval;

  if (busy) return;

  el.hero.className = 'hero';
  if (s.internet) {
    el.hero.classList.add('is-ok');
    el.heroTitle.textContent = '网络正常';
    el.heroSub.textContent = `探测点 ${s.probe} 可达` +
      (st === 'on' && s.outport ? ` · 出口 ${s.outport}` : '');
  } else {
    el.hero.classList.add('is-down');
    el.heroTitle.textContent = '外网不通';
    if (st === 'on') {
      el.heroSub.textContent = '门户显示在线但实际没网（僵尸会话）· 点「一键连接」';
    } else if (!s.ready) {
      el.heroSub.textContent = '还没配置账号密码 · 去「设置」填一下';
    } else if (s.portal_error) {
      el.heroSub.textContent = '门户访问失败：' + s.portal_error;
    } else {
      el.heroSub.textContent = (s.probe || '需要重新认证') + ' · 点「一键连接」';
    }
  }
}

async function refresh() {
  if (!invoke) return;
  try {
    renderStatus(await invoke('status'));
  } catch (e) {
    toast('读取状态失败：' + e, 'err');
  }
}

/* ------------------------------------------------------------------ */
/* 日志                                                                */
/* ------------------------------------------------------------------ */

const MAX_LINES = 400;

function addLog(ev) {
  if (el.log.querySelector('.log-empty')) el.log.innerHTML = '';
  const div = document.createElement('div');
  div.className = 'log-line ' + (ev.level || 'info');
  const t = document.createElement('span');
  t.className = 'log-t';
  t.textContent = hhmm(ev.ts);
  const m = document.createElement('span');
  m.className = 'log-m';
  m.textContent = ev.msg;
  div.append(t, m);
  el.log.appendChild(div);
  while (el.log.childElementCount > MAX_LINES) el.log.removeChild(el.log.firstChild);
  if (el.autoScroll.checked) el.log.scrollTop = el.log.scrollHeight;
}

async function wireEvents() {
  if (!listen) return;
  el.dot.className = 'dot on';
  await listen('cf-log', (e) => {
    const ev = e.payload;
    addLog(ev);
    // 认证类日志出现后状态大概率变了，顺手刷新
    if (/^\[(ok|!!)\]\s*(认证|注销|自动重连|连接成功|已注销)/.test(ev.msg)) {
      setTimeout(refresh, 400);
    }
  });
}

/* ------------------------------------------------------------------ */
/* 动作                                                                */
/* ------------------------------------------------------------------ */

async function runFix() {
  setBusy(true, '正在连接…');
  try {
    const r = await invoke('do_fix');
    toast(r.msg, r.ok ? 'ok' : 'err');
  } catch (e) {
    toast('连接失败：' + e, 'err');
  } finally {
    setBusy(false);
    refresh();
  }
}

async function runLogout() {
  if (!confirm('确认注销当前校园网会话？注销后会短暂断网。')) return;
  setBusy(true, '正在注销…');
  try {
    const r = await invoke('do_logout');
    toast(r.msg, r.ok ? 'ok' : 'err');
  } catch (e) {
    toast('注销失败：' + e, 'err');
  } finally {
    setBusy(false);
    refresh();
  }
}

/* ------------------------------------------------------------------ */
/* 运营商下拉                                                          */
/* ------------------------------------------------------------------ */

// 门户返回的固定四个出口。配置里如果存着列表外的值（比如学校以后加了新运营商），
// 也把它补进去，不会静默丢掉。
const CHANNELS = ['校园网', '中国移动', '中国电信', '中国联通'];

function fillChannels(current) {
  const list = CHANNELS.slice();
  if (current && !list.includes(current)) list.push(current);
  el.fChannel.innerHTML = '';
  for (const name of list) {
    const o = document.createElement('option');
    o.value = name;
    o.textContent = name;
    el.fChannel.appendChild(o);
  }
  el.fChannel.value = current || '中国电信';
}

/* ------------------------------------------------------------------ */
/* 检查方式                                                            */
/* ------------------------------------------------------------------ */

const MODE_HINT = {
  event: '换 WiFi、休眠唤醒、插拔网线时立即检查，平时不打扰。'
       + '注意：校园网会话静默过期（IP 没变）不会触发，这种情况可能漏检。',
  poll: '每过一段时间主动探测一次，不会漏检，但有周期性网络请求。',
};

function updateModeUI() {
  const m = el.checkMode.value;
  el.modeHint.textContent = MODE_HINT[m] || '';
  el.intervalWrap.style.display = m === 'poll' ? '' : 'none';
}

/* 自动退出只有在开了开机自启时才有意义，否则置灰 */
function updateAutoExitState() {
  const on = el.fAutostart.checked;
  el.fAutoExit.disabled = !on;
  el.autoExitRow.style.opacity = on ? '' : '0.5';
}

/*
 * 「保存配置」按钮的可点状态。
 *
 * 记住上次保存时的表单快照，和当前值对比：
 *   一样 -> 按钮置灰不可点（没什么可存的）
 *   不一样 -> 变蓝可点
 *
 * 密码框永远不预填，所以只要有输入就算“改了”。
 */
let savedForm = null;

function formSnapshot() {
  return JSON.stringify({
    username: el.fUser.value.trim(),
    channel: el.fChannel.value,
    autostart: el.fAutostart.checked,
    auto_exit: el.fAutoExit.checked,
    has_password: el.fPass.value !== '',
  });
}

function markFormSaved() {
  savedForm = formSnapshot();
  updateSaveButton();
}

function updateSaveButton() {
  const dirty = savedForm !== null && formSnapshot() !== savedForm;
  el.btnSave.disabled = !dirty;
}

/* ------------------------------------------------------------------ */
/* 配置                                                                */
/* ------------------------------------------------------------------ */

async function loadConfig() {
  try {
    const c = await invoke('get_config');
    portalURL = c.portal || portalURL;
    el.fUser.value = c.username || '';
    fillChannels(c.channel);
    el.checkMode.value = c.check_mode || 'event';
    updateModeUI();
    el.watchInterval.value = c.interval || 30;
    el.fAutostart.checked = !!c.autostart;
    el.fAutoExit.checked = !!c.auto_exit;
    updateAutoExitState();
    markFormSaved();
    el.pwdNote.textContent = c.has_password ? '(已保存)' : '(未设置)';
  } catch (e) {
    toast('读取配置失败：' + e, 'err');
  }

  try {
    const a = await invoke('app_info');
    el.about.innerHTML = '';
    const rows = [
      ['版本', 'v' + a.version],
      ['门户', a.portal],
      ['SSID', a.profile],
      ['配置', a.config_path],
    ];
    for (const [k, v] of rows) {
      const d = document.createElement('div');
      const s1 = document.createElement('span'); s1.textContent = k;
      const s2 = document.createElement('span'); s2.textContent = v;
      d.append(s1, s2);
      el.about.appendChild(d);
    }
  } catch { /* 忽略 */ }
}

async function saveConfig(e) {
  e.preventDefault();
  const patch = {
    username: el.fUser.value.trim(),
    channel: el.fChannel.value,
    autostart: el.fAutostart.checked,
    auto_exit: el.fAutoExit.checked,
  };
  if (el.fPass.value) patch.password = el.fPass.value;
  if (!patch.username) {
    el.cfgMsg.textContent = '用户名不能为空';
    el.cfgMsg.className = 'form-msg err';
    return;
  }
  try {
    const r = await invoke('save_config', { patch });
    el.cfgMsg.textContent = r.msg || '已保存';
    el.cfgMsg.className = 'form-msg ' + (r.ok ? 'ok' : 'err');
    if (r.ok) {
      el.fPass.value = '';
      markFormSaved();
      toast('配置已保存', 'ok');
      await loadConfig();
      await refresh();
    }
  } catch (e) {
    el.cfgMsg.textContent = '保存失败：' + e;
    el.cfgMsg.className = 'form-msg err';
  }
}

/* ------------------------------------------------------------------ */
/* 绑定                                                                */
/* ------------------------------------------------------------------ */

el.btnFix.addEventListener('click', runFix);
el.btnRefresh.addEventListener('click', async () => { await refresh(); toast('已刷新'); });
el.btnLogout.addEventListener('click', runLogout);

el.btnPortal.addEventListener('click', async () => {
  try { await invoke('open_portal'); toast('已在浏览器打开认证页'); }
  catch (e) { toast('打开失败：' + e, 'err'); }
});

el.btnQuit.addEventListener('click', async () => {
  if (confirm('确认退出 CampusFlow？退出后自动重连也会停止。')) {
    await invoke('quit_app');
  }
});

el.btnClearLog.addEventListener('click', async () => {
  el.log.innerHTML = '<div class="log-empty">日志已清空</div>';
  try { await invoke('clear_logs'); } catch { /* 忽略 */ }
});

el.cfgForm.addEventListener('submit', saveConfig);

el.fAutostart.addEventListener('change', updateAutoExitState);

// 任何输入变化都重新算一次保存按钮该不该可点
el.cfgForm.addEventListener('input', updateSaveButton);
el.cfgForm.addEventListener('change', updateSaveButton);

el.watchToggle.addEventListener('change', async () => {
  const on = el.watchToggle.checked;
  const interval = parseInt(el.watchInterval.value, 10) || 30;
  try {
    const r = await invoke('watch_control', { action: on ? 'start' : 'stop', interval });
    toast(r.msg, r.ok ? 'ok' : 'err');
  } catch (e) {
    toast('操作失败：' + e, 'err');
    el.watchToggle.checked = !on;
  }
  setTimeout(refresh, 400);
});

el.watchInterval.addEventListener('change', async () => {
  const interval = parseInt(el.watchInterval.value, 10) || 30;
  try {
    // action=set：没在跑就只存配置，在跑就重启让它生效
    const r = await invoke('watch_control', { action: 'set', interval });
    toast(r.msg, r.ok ? 'ok' : 'err');
  } catch (e) {
    toast('设置失败：' + e, 'err');
  }
  setTimeout(refresh, 400);
});

el.checkMode.addEventListener('change', async () => {
  updateModeUI();
  try {
    const r = await invoke('save_config', {
      patch: { check_mode: el.checkMode.value },
    });
    toast(r.msg, r.ok ? 'ok' : 'err');
  } catch (e) {
    toast('保存失败：' + e, 'err');
  }
  setTimeout(refresh, 400);
});

document.addEventListener('keydown', (e) => {
  if (e.target.matches('input, textarea')) return;
  if (e.key === 'r' || e.key === 'R') refresh();
  if (e.key === 'f' || e.key === 'F') { if (!busy) runFix(); }
});

/* ------------------------------------------------------------------ */
/* 启动                                                                */
/* ------------------------------------------------------------------ */

(async function init() {
  if (!invoke) {
    document.body.innerHTML =
      '<div style="padding:24px;font-family:sans-serif;color:#e7ecf3">' +
      '这个页面需要通过 CampusFlow 桌面程序打开（Tauri 运行时未就绪）。</div>';
    return;
  }

  el.log.innerHTML = '<div class="log-empty">等待事件…</div>';

  // 先拉历史日志，再挂事件监听，避免中间丢事件
  try {
    const logs = await invoke('logs');
    if (logs && logs.length) {
      el.log.innerHTML = '';
      logs.forEach(addLog);
    }
  } catch { /* 忽略 */ }

  await wireEvents();
  await loadConfig();
  await refresh();

  // 兜底轮询：后台线程改了状态，界面也能跟上
  setInterval(refresh, 15000);
})();

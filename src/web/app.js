let torrents = [];
let stats = {};
let authToken = localStorage.getItem('seeder_token') || null;

const themeIcons = { auto: 'adjust', light: 'sun', dark: 'moon' };

function applyTheme(theme) {
  const dark = theme === 'dark' ||
    (theme === 'auto' && window.matchMedia('(prefers-color-scheme:dark)').matches);
  document.documentElement.classList.toggle('dark-mode', dark);
}

function setTheme(theme) {
  localStorage.setItem('seeder_theme', theme);
  applyTheme(theme);
  const sel = document.getElementById('theme-select');
  const ico = document.getElementById('theme-icon');
  if (sel) sel.value = theme;
  if (ico) ico.className = themeIcons[theme] + ' icon';
  updateChartColors();
}

window.matchMedia('(prefers-color-scheme:dark)').addEventListener('change', () => {
  const t = localStorage.getItem('seeder_theme') || 'auto';
  if (t === 'auto') { applyTheme('auto'); updateChartColors(); }
});

function toggleMobileMenu() {
  document.getElementById('header-actions').classList.toggle('open');
}

function closeMobileMenu() {
  document.getElementById('header-actions').classList.remove('open');
}

document.addEventListener('click', function(e) {
  const menu = document.getElementById('header-actions');
  const btn  = document.getElementById('hamburger-btn');
  if (menu.classList.contains('open') && !menu.contains(e.target) && !btn.contains(e.target)) {
    closeMobileMenu();
  }
});

document.getElementById('header-actions').addEventListener('click', function(e) {
  if (e.target.closest('button')) setTimeout(closeMobileMenu, 120);
});

function authHeaders() {
  const h = {'Content-Type': 'application/json'};
  if (authToken) h['Authorization'] = 'Bearer ' + authToken;
  return h;
}

async function apiFetch(url, opts = {}) {
  opts.headers = {...(opts.headers || {}), ...authHeaders()};
  const r = await fetch(url, opts);
  if (r.status === 401) {
    authToken = null;
    localStorage.removeItem('seeder_token');
    disconnectWs();
    showLogin();
    throw new Error('Unauthorized');
  }
  return r;
}

async function showLogin() {
  await loadAuthInfo();
  const overlay = document.getElementById('login-overlay');
  const alreadyVisible = overlay.style.display === 'flex';
  overlay.style.display = 'flex';
  document.getElementById('main-app').style.display = 'none';
  if (!alreadyVisible) {
    document.getElementById('login-password').value = '';
    document.getElementById('login-totp').value = '';
    document.getElementById('login-error').style.display = 'none';
    setTimeout(() => document.getElementById('login-password').focus(), 100);
  }
}

function showApp() {
  document.getElementById('login-overlay').style.display = 'none';
  document.getElementById('main-app').style.display = 'block';
  const hasAuth = authToken && authToken !== 'noauth';
  document.getElementById('logout-btn').style.display = hasAuth ? '' : 'none';
  if (!chartPeers) initChart();
  connectWs();
}

async function doLogin() {
  const pw = document.getElementById('login-password').value;
  const totp = document.getElementById('login-totp').value.trim();
  try {
    const r = await fetch('/api/login', {
      method: 'POST',
      headers: {'Content-Type': 'application/json'},
      body: JSON.stringify({password: pw, totp_code: totp || null}),
    });
    if (r.ok) {
      const data = await r.json();
      authToken = data.token;
      localStorage.setItem('seeder_token', authToken);
      showApp();
      await loadTorrents();
      await loadStats();
    } else {
      const errData = await r.json().catch(() => ({}));
      if (errData.requires_totp) {
        authInfo.requires_totp = true;
        document.getElementById('totp-field').style.display = '';
        document.getElementById('login-error').textContent = 'Please enter your authenticator code.';
        document.getElementById('login-error').style.display = 'block';
        setTimeout(() => document.getElementById('login-totp').focus(), 50);
      } else {
        document.getElementById('login-error').textContent = errData.error || 'Login failed.';
        document.getElementById('login-error').style.display = 'block';
      }
    }
  } catch(e) {
    document.getElementById('login-error').textContent = 'Login failed: ' + e.message;
    document.getElementById('login-error').style.display = 'block';
  }
}

async function doLogout() {
  disconnectWs();
  try {
    await fetch('/api/logout', {
      method: 'POST',
      headers: authHeaders(),
    });
  } catch(_) {}
  authToken = null;
  localStorage.removeItem('seeder_token');
  showLogin();
}

document.getElementById('login-password').addEventListener('keydown', function(e) {
  if (e.key === 'Enter') doLogin();
});

document.getElementById('login-totp').addEventListener('keydown', function(e) {
  if (e.key === 'Enter') doLogin();
});

let authInfo = {requires_password: true, requires_totp: false};

async function loadAuthInfo() {
  try {
    const r = await fetch('/api/auth-info');
    authInfo = await r.json();
  } catch(_) {}
  document.getElementById('totp-field').style.display =
    authInfo.requires_totp ? '' : 'none';
}

async function init() {
  if (authToken) {
    try {
      const r = await fetch('/api/status', {headers: authHeaders()});
      if (r.ok) {
        showApp();
        await loadTorrents();
        const data = await r.json();
        stats = data.torrents || {};
        document.getElementById('status-text').textContent =
          'Active seeders: ' + Object.keys(stats).length;
        renderTable();
        return;
      }
    } catch(_) {}
  }
  await loadAuthInfo();
  if (!authInfo.requires_password && !authInfo.requires_totp) {
    try {
      const r = await fetch('/api/login', {
        method: 'POST',
        headers: {'Content-Type': 'application/json'},
        body: JSON.stringify({password: '', totp_code: null}),
      });
      if (r.ok) {
        const data = await r.json();
        if (data.token === 'noauth') {
          authToken = 'noauth';
          showApp();
          await loadTorrents();
          await loadStats();
          return;
        }
      }
    } catch(_) {}
  }
  showLogin();
}

init();

function fmtBytes(n) {
  if (n < 1024) return n + ' B';
  if (n < 1024*1024) return (n/1024).toFixed(1) + ' KB';
  if (n < 1024*1024*1024) return (n/1024/1024).toFixed(1) + ' MB';
  return (n/1024/1024/1024).toFixed(2) + ' GB';
}

function escHtml(s) {
  return String(s).replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;');
}

function escAttr(s) {
  return String(s).replace(/&/g,'&amp;').replace(/"/g,'&quot;');
}

let ws = null;
let wsReconnectTimeout = null;
let wsReconnectDelay = 1000;
const WS_RECONNECT_MAX = 30000;

function connectWs() {
  if (!authToken) return;
  if (ws && (ws.readyState === WebSocket.CONNECTING || ws.readyState === WebSocket.OPEN)) return;
  const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
  const token = (authToken && authToken !== 'noauth') ? '?token=' + encodeURIComponent(authToken) : '';
  const url = proto + '//' + location.host + '/api/ws' + token;
  try {
    ws = new WebSocket(url);
  } catch(e) {
    scheduleWsReconnect();
    return;
  }
  ws.onopen = function() {
    wsReconnectDelay = 1000;
    document.getElementById('ws-indicator').textContent = '● live';
    document.getElementById('ws-indicator').style.color = '#21ba45';
  };
  ws.onmessage = function(e) {
    try {
      const msg = JSON.parse(e.data);
      if (msg.type === 'stats') handleStatsMsg(msg);
      else if (msg.type === 'log') handleLogMsg(msg);
    } catch(_) {}
  };
  ws.onclose = function() {
    ws = null;
    document.getElementById('ws-indicator').textContent = '○ disconnected';
    document.getElementById('ws-indicator').style.color = '#aaa';
    if (authToken) scheduleWsReconnect();
  };
  ws.onerror = function() {
    if (ws) ws.close();
  };
}

function scheduleWsReconnect() {
  clearTimeout(wsReconnectTimeout);
  wsReconnectTimeout = setTimeout(function() {
    if (authToken) connectWs();
  }, wsReconnectDelay);
  wsReconnectDelay = Math.min(wsReconnectDelay * 2, WS_RECONNECT_MAX);
}

function disconnectWs() {
  clearTimeout(wsReconnectTimeout);
  document.getElementById('ws-indicator').textContent = '';
  if (ws) {
    ws.onclose = null;
    ws.close();
    ws = null;
  }
}

let chartPeers = null;
let chartRate = null;
let wsData = [];
let wsRange = 24;
const WS_MAX_HOURS = 72;

function getChartColors() {
  const dark = document.documentElement.classList.contains('dark-mode');
  return {
    peers: dark ? 'rgba(91, 192, 255, 0.9)' : 'rgba(33, 133, 208, 0.9)',
    rate: dark ? 'rgba(0, 210, 150, 0.9)' : 'rgba(33, 186, 69, 0.9)',
    grid: dark ? 'rgba(255,255,255,0.08)' : 'rgba(0,0,0,0.07)',
    text: dark ? '#9aa' : '#666',
  };
}

function makeChartOptions(c, yLabel, yTickCb) {
  return {
    animation: false,
    responsive: true,
    maintainAspectRatio: false,
    interaction: { mode: 'index', intersect: false },
    plugins: {
      legend: { display: false },
      tooltip: {
        callbacks: {
          label: function(ctx) { return yLabel + ': ' + (yTickCb ? yTickCb(ctx.parsed.y) : ctx.parsed.y); },
        },
      },
    },
    scales: {
      x: {
        type: 'time',
        time: {
          tooltipFormat: 'HH:mm:ss',
          displayFormats: { second: 'HH:mm', minute: 'HH:mm', hour: 'HH:mm' },
        },
        ticks: { color: c.text, maxTicksLimit: 8, font: { size: 10 } },
        grid: { color: c.grid },
      },
      y: {
        type: 'linear',
        position: 'left',
        ticks: {
          color: c.text,
          precision: 0,
          font: { size: 10 },
          callback: yTickCb || undefined,
        },
        grid: { color: c.grid },
        min: 0,
      },
    },
  };
}

function initChart() {
  const c = getChartColors();

  chartPeers = new Chart(
    document.getElementById('chart-peers').getContext('2d'),
    {
      type: 'line',
      data: {
        datasets: [{
          data: [],
          borderColor: c.peers,
          backgroundColor: 'transparent',
          borderWidth: 1.5,
          pointRadius: 0,
          tension: 0.15,
        }],
      },
      options: makeChartOptions(c, 'Peers', null),
    }
  );

  chartRate = new Chart(
    document.getElementById('chart-rate').getContext('2d'),
    {
      type: 'line',
      data: {
        datasets: [{
          data: [],
          borderColor: c.rate,
          backgroundColor: 'transparent',
          borderWidth: 1.5,
          pointRadius: 0,
          tension: 0.15,
        }],
      },
      options: makeChartOptions(c, 'Upload', function(v) { return fmtBytes(v) + '/s'; }),
    }
  );
}

function applyColorsToChart(ch, borderColor, c) {
  if (!ch) return;
  ch.data.datasets[0].borderColor = borderColor;
  const opts = ch.options;
  opts.scales.x.ticks.color = c.text;
  opts.scales.x.grid.color = c.grid;
  opts.scales.y.ticks.color = c.text;
  opts.scales.y.grid.color = c.grid;
  ch.update('none');
}

function updateChartColors() {
  const c = getChartColors();
  applyColorsToChart(chartPeers, c.peers, c);
  applyColorsToChart(chartRate, c.rate, c);
}

function downsample(arr, maxPoints) {
  if (arr.length <= maxPoints) return arr;
  const step = arr.length / maxPoints;
  const result = [];
  for (let i = 0; i < maxPoints; i++) {
    result.push(arr[Math.floor(i * step)]);
  }
  return result;
}

function setChartRange(hours) {
  wsRange = hours;
  document.querySelectorAll('#chart-range-btns .button').forEach(function(btn, i) {
    const h = [24, 48, 72][i];
    btn.classList.toggle('active', h === hours);
  });
  updateChart();
}

function updateChart() {
  if (!chartPeers && !chartRate) return;
  const now = Date.now() / 1000;
  const cutoff = now - wsRange * 3600;
  let visible = wsData;
  if (wsData.length > 0 && wsData[0].ts < cutoff) {
    let lo = 0, hi = wsData.length;
    while (lo < hi) {
      const mid = (lo + hi) >> 1;
      if (wsData[mid].ts < cutoff) lo = mid + 1; else hi = mid;
    }
    visible = wsData.slice(lo);
  }
  const sampled = downsample(visible, 1440);
  if (chartPeers) {
    chartPeers.data.datasets[0].data = sampled.map(function(d) { return { x: d.ts * 1000, y: d.peers }; });
    chartPeers.update('none');
  }
  if (chartRate) {
    chartRate.data.datasets[0].data = sampled.map(function(d) { return { x: d.ts * 1000, y: d.rate }; });
    chartRate.update('none');
  }
}

let expandedRows = new Set();

function updateTableStats() {
  torrents.forEach((t, i) => {
    const name = t.name || (t.file && t.file[0]) || t.torrent_file || t.magnet || 'torrent-' + i;
    const st = stats[name] || {};
    const uploaded = st.uploaded !== undefined ? fmtBytes(st.uploaded) : '—';
    const peers = st.peer_count !== undefined ? st.peer_count : '—';
    const peersCell = document.getElementById(`peers-${i}`);
    const uploadedCell = document.getElementById(`uploaded-${i}`);
    if (peersCell) peersCell.textContent = peers;
    if (uploadedCell) uploadedCell.textContent = uploaded;
  });
  const totalPeers = Object.values(stats).reduce((sum, s) => sum + (s.peer_count || 0), 0);
  const totalRate = Object.values(stats).reduce((sum, s) => sum + (s.uploaded || 0), 0);
  if (!ws || ws.readyState !== WebSocket.OPEN) {
    document.getElementById('status-text').textContent =
      'Peers: ' + totalPeers + ' Upload: ' + fmtBytes(totalRate) + '/s';
  }
}

function handleStatsMsg(msg) {
  wsData.push({ ts: msg.ts, peers: msg.peers, rate: msg.rate });
  const cutoff = msg.ts - WS_MAX_HOURS * 3600;
  while (wsData.length > 0 && wsData[0].ts < cutoff) wsData.shift();
  updateChart();
  if (msg.torrents) {
    stats = msg.torrents;
    updateTableStats();
  }
  document.getElementById('status-text').textContent =
    'Peers: ' + msg.peers + ' Upload: ' + fmtBytes(msg.rate) + '/s';
}

let consoleLines = [];
const MAX_CONSOLE_LINES = 10000;

function handleLogMsg(msg) {
  consoleLines.push(msg.line);
  if (consoleLines.length > MAX_CONSOLE_LINES) consoleLines.shift();
  const el = document.getElementById('console-output');
  if (el && $('#console-modal').hasClass('active')) {
    appendConsoleLine(el, msg.line);
  }
}

function appendConsoleLine(el, line) {
  const div = document.createElement('div');
  div.textContent = line;
  el.appendChild(div);
  while (el.children.length > MAX_CONSOLE_LINES) el.removeChild(el.firstChild);
  if (document.getElementById('console-autoscroll').checked) {
    el.scrollTop = el.scrollHeight;
  }
}

function openConsole() {
  const el = document.getElementById('console-output');
  el.innerHTML = '';
  for (let i = 0; i < consoleLines.length; i++) {
    const div = document.createElement('div');
    div.textContent = consoleLines[i];
    el.appendChild(div);
  }
  $('#console-modal').modal({
    onVisible: function() {
      if (document.getElementById('console-autoscroll').checked) {
        el.scrollTop = el.scrollHeight;
      }
    },
  }).modal('show');
}

function clearConsole() {
  consoleLines = [];
  document.getElementById('console-output').innerHTML = '';
}

async function loadTorrents() {
  try {
    const r = await apiFetch('/api/torrents');
    torrents = await r.json();
    renderTable();
  } catch(e) {
    if (e.message !== 'Unauthorized') {
      document.getElementById('torrent-tbody').innerHTML =
        '<tr><td colspan="5" class="center aligned negative">Failed to load torrents: ' + e.message + '</td></tr>';
    }
  }
}

async function loadStats() {
  if (!authToken) return;
  try {
    const r = await apiFetch('/api/status');
    const data = await r.json();
    stats = data.torrents || {};
    updateTableStats();
  } catch(_) {}
}

function renderTable() {
  if (torrents.length === 0) {
    document.getElementById('torrent-tbody').innerHTML =
      '<tr><td colspan="5" class="center aligned">No torrents configured.</td></tr>';
    return;
  }
  let html = '';
  torrents.forEach((t, i) => {
    const name = t.name || (t.file && t.file[0]) || t.torrent_file || t.magnet || 'torrent-' + i;
    const st = stats[name] || {};
    const uploaded = st.uploaded !== undefined ? fmtBytes(st.uploaded) : '—';
    const peers = st.peer_count !== undefined ? st.peer_count : '—';
    const enabledLabel = t.enabled
      ? '<span class="ui green label">Yes</span>'
      : '<span class="ui grey label">No</span>';
    const limit = t.upload_limit ? t.upload_limit + ' KB/s' : '<em>unlimited</em>';
    const version = t.version || 'v1';
    const proto = (t.protocol || 'both').toLowerCase();
    const protoClass = proto === 'bt' ? 'proto-bt' : proto === 'rtc' ? 'proto-rtc' : 'proto-both';
    const protoLabel = `<span class="ui tiny label ${protoClass}">${escHtml(proto)}</span>`;
    const isExpanded = expandedRows.has(i);
    const iconClass = isExpanded ? 'chevron down' : 'chevron right';
    html += `<tr data-torrent-idx="${i}" style="${t.enabled ? '' : 'opacity:0.55'}">
      <td style="width:1px;padding-right:0;padding-left:4px">
        <button class="torrent-expand-btn" onclick="toggleDetail(${i})" 
                title="${isExpanded ? 'Hide details' : 'Show details'}"
                data-idx="${i}">
          <i id="expand-icon-${i}" class="${iconClass} icon"></i>
        </button>
      </td>
      <td><strong>${escHtml(name)}</strong></td>
      <td id="peers-${i}">${peers}</td>
      <td id="uploaded-${i}">${uploaded}</td>
      <td>
        <button class="ui mini button" onclick="toggleEnabled(${i})">
          ${t.enabled ? 'Disable' : 'Enable'}
        </button>
        <button class="ui mini red button" onclick="deleteTorrent(${i})">
          <i class="trash icon"></i>Delete
        </button>
      </td>
    </tr>
    <tr id="detail-row-${i}" class="detail-row" style="display:${isExpanded ? '' : 'none'}">
      <td></td>
      <td colspan="4">
        <div class="torrent-detail-content">
          <span><strong>Protocol:</strong>&nbsp;${protoLabel}</span>
          <span><strong>Version:</strong>&nbsp;<span class="ui tiny label">${escHtml(version)}</span></span>
          <span><strong>Enabled:</strong>&nbsp;${enabledLabel}</span>
          <span><strong>Upload Limit:</strong>&nbsp;${limit}</span>
          ${t.file && t.file.length ? `<span><strong>Files:</strong>&nbsp;${t.file.map(f => '<code class="detail-code">' + escHtml(f) + '</code>').join(', ')}</span>` : ''}
          ${t.torrent_file ? `<span><strong>Torrent:</strong>&nbsp;<code class="detail-code">${escHtml(t.torrent_file)}</code></span>` : ''}
          ${t.trackers && t.trackers.length ? `<span><strong>Trackers:</strong>&nbsp;${t.trackers.map(tr => '<code class="detail-code detail-tracker">' + escHtml(tr) + '</code>').join(', ')}</span>` : ''}
          ${t.create_torrent ? `<span><strong>Create Torrent:</strong>&nbsp;<span class="ui tiny label">Yes</span></span>` : ''}
          ${t.private ? `<span><strong>Private:</strong>&nbsp;<span class="ui tiny orange label">Yes</span></span>` : ''}
        </div>
      </td>
    </tr>`;
  });
  document.getElementById('torrent-tbody').innerHTML = html;
}
function toggleDetail(i) {
  const row = document.getElementById('detail-row-' + i);
  const icon = document.getElementById('expand-icon-' + i);
  const isOpen = row.style.display !== 'none';
  if (isOpen) {
    row.style.display = 'none';
    icon.className = 'chevron right icon';
    expandedRows.delete(i);
  } else {
    row.style.display = '';
    icon.className = 'chevron down icon';
    expandedRows.add(i);
  }
  icon.style.margin = '0';
}

function toggleCreateTorrent() {
  const createTorrent = document.getElementById('f-create-torrent').checked;
  const versionSelect = document.getElementById('f-version');
  const privateContainer = document.getElementById('f-private-container');
  if (createTorrent) {
    versionSelect.disabled = false;
    privateContainer.style.display = 'block';
  } else {
    versionSelect.disabled = true;
    privateContainer.style.display = 'none';
  }
}

function toggleAddForm() {
  const f = document.getElementById('add-form');
  f.style.display = f.style.display === 'none' ? 'block' : 'none';
}

async function addTorrent() {
  const path = document.getElementById('f-path').value.trim();
  const trackers = document.getElementById('f-trackers').value.split('\n').map(s=>s.trim()).filter(Boolean);
  const iceRaw = document.getElementById('f-ice').value.trim();
  const iceServers = iceRaw ? iceRaw.split('\n').map(s=>s.trim()).filter(Boolean) : null;
  const rtcInterval = parseInt(document.getElementById('f-rtc-interval').value) || null;
  const protocol = document.getElementById('f-protocol').value;
  const entry = {
    name: document.getElementById('f-name').value.trim() || null,
    file: path ? [path] : [],
    trackers,
    torrent_file: document.getElementById('f-torrent-file').value.trim() || null,
    create_torrent: document.getElementById('f-create-torrent').checked,
    magnet: document.getElementById('f-magnet').value.trim() || null,
    enabled: document.getElementById('f-enabled').checked,
    upload_limit: parseInt(document.getElementById('f-upload-limit').value) || null,
    webseed: null,
    version: document.getElementById('f-version').value,
    protocol: protocol !== 'both' ? protocol : null,
    ice: iceServers,
    rtc_interval: rtcInterval,
    private: document.getElementById('f-private').checked || false,
    allowed_extensions: (() => {
      const ext = document.getElementById('f-allowed-extensions').value.trim();
      return ext ? ext.split(',').map(s=>s.trim()).filter(Boolean) : null;
    })(),
  };
  try {
    const r = await apiFetch('/api/torrents', {
      method: 'POST',
      body: JSON.stringify(entry),
    });
    if (r.ok) {
      const data = await r.json();
      if (data.using_source_folder) {
        const infoMsg = document.createElement('div');
        infoMsg.className = 'ui info message';
        infoMsg.style.marginTop = '12px';
        infoMsg.innerHTML = `<i class="info circle icon"></i> Using Source Folder from Global Settings: <strong>${data.source_folder}</strong>`;
        const form = document.getElementById('form-add');
        form.appendChild(infoMsg);
        setTimeout(() => infoMsg.remove(), 5000);
      }
      toggleAddForm();
      await loadTorrents();
    } else {
      alert('Failed to add torrent: ' + await r.text());
    }
  } catch(e) {
    if (e.message !== 'Unauthorized') alert('Error: ' + e.message);
  }
}
async function toggleEnabled(i) {
  const t = {...torrents[i], enabled: !torrents[i].enabled};
  try {
    const r = await apiFetch('/api/torrents/' + i, {
      method: 'PUT',
      body: JSON.stringify(t),
    });
    if (r.ok) {
      await loadTorrents();
    } else {
      alert('Failed to update torrent: ' + await r.text());
    }
  } catch(e) {
    if (e.message !== 'Unauthorized') alert('Error: ' + e.message);
  }
}

async function deleteTorrent(i) {
  if (!confirm('Delete torrent entry ' + i + '?')) return;
  try {
    const r = await apiFetch('/api/torrents/' + i, { method: 'DELETE' });
    if (r.ok) {
      await loadTorrents();
    } else {
      alert('Failed to delete: ' + await r.text());
    }
  } catch(e) {
    if (e.message !== 'Unauthorized') alert('Error: ' + e.message);
  }
}

async function openSettings() {
  try {
    const r = await apiFetch('/api/config');
    const cfg = await r.json();
    document.getElementById('s-listen-port').value = cfg.listen_port || 6881;
    document.getElementById('s-upnp').checked = cfg.upnp === true;
    document.getElementById('s-protocol').value = cfg.protocol || 'both';
    document.getElementById('s-rtc-ice').value = (cfg.rtc_ice_servers || []).join('\n');
    document.getElementById('s-rtc-interval').value = cfg.rtc_interval_ms || 5000;
    document.getElementById('s-web-password').value = cfg.web_password || '';
    document.getElementById('s-web-cert').value = cfg.web_cert || '';
    document.getElementById('s-web-key').value = cfg.web_key || '';
    document.getElementById('s-le-domain').value = cfg.letsencrypt_domain || '';
    document.getElementById('s-le-email').value = cfg.letsencrypt_email || '';
    document.getElementById('s-le-port').value = cfg.letsencrypt_http_port || '';
    document.getElementById('s-le-expiry').value = cfg.le_cert_expiry || '';
    document.getElementById('s-login-rate-limit').value = cfg.web_login_rate_limit ?? '';
    const twoFAEnabled = !!cfg.totp_enabled;
    const totpMsg = document.getElementById('totp-status-msg');
    totpMsg.className = 'ui message ' + (twoFAEnabled ? 'positive' : 'warning');
    totpMsg.innerHTML = twoFAEnabled
      ? '<i class="check circle icon"></i> Two-factor authentication is <strong>enabled</strong>.'
      : '<i class="warning sign icon"></i> Two-factor authentication is <strong>disabled</strong>.';
    document.getElementById('btn-2fa-enable').style.display = twoFAEnabled ? 'none' : '';
    document.getElementById('btn-2fa-disable').style.display = twoFAEnabled ? '' : 'none';
    document.getElementById('s-log-level').value = cfg.log_level || 'info';
    document.getElementById('s-show-stats').checked = cfg.show_stats !== false;
    document.getElementById('s-seeder-threads').value = cfg.seeder_threads || '';
    document.getElementById('s-web-threads').value = cfg.web_threads || '';
    document.getElementById('s-source-folder').value = cfg.source_folder || '';
    if (cfg.proxy) {
      document.getElementById('s-proxy-type').value = cfg.proxy.proxy_type || '';
      document.getElementById('s-proxy-host').value = cfg.proxy.host || '';
      document.getElementById('s-proxy-port').value = cfg.proxy.port || '';
      document.getElementById('s-proxy-user').value = cfg.proxy.username || '';
      document.getElementById('s-proxy-pass').value = cfg.proxy.password || '';
    } else {
      document.getElementById('s-proxy-type').value = '';
      document.getElementById('s-proxy-host').value = '';
      document.getElementById('s-proxy-port').value = '';
      document.getElementById('s-proxy-user').value = '';
      document.getElementById('s-proxy-pass').value = '';
    }
  } catch(e) {
    if (e.message !== 'Unauthorized') alert('Failed to load settings: ' + e.message);
    return;
  }
  $('#settings-modal').modal('show');
}

async function saveSettings() {
  const proxyType = document.getElementById('s-proxy-type').value;
  const proxyHost = document.getElementById('s-proxy-host').value.trim();
  const proxyPort = parseInt(document.getElementById('s-proxy-port').value) || 0;
  const hasProxy = proxyType && proxyHost && proxyPort;
  const listenPort = parseInt(document.getElementById('s-listen-port').value) || 6881;
  const webPassword = document.getElementById('s-web-password').value;
  const webCert = document.getElementById('s-web-cert').value.trim();
  const webKey = document.getElementById('s-web-key').value.trim();
  const rtcIceRaw = document.getElementById('s-rtc-ice').value.trim();
  const rtcIceServers = rtcIceRaw ? rtcIceRaw.split('\n').map(s=>s.trim()).filter(Boolean) : null;
  const rtcInterval = parseInt(document.getElementById('s-rtc-interval').value) || null;
  const proto = document.getElementById('s-protocol').value;
  const cfg = {
    listen_port: listenPort,
    web_port: null,
    web_password: webPassword || null,
    web_cert: webCert || null,
    web_key: webKey || null,
    protocol: proto !== 'both' ? proto : null,
    rtc_ice_servers: rtcIceServers,
    rtc_interval_ms: rtcInterval,
    proxy: hasProxy ? {
      proxy_type: proxyType,
      host: proxyHost,
      port: proxyPort,
      username: document.getElementById('s-proxy-user').value.trim() || null,
      password: document.getElementById('s-proxy-pass').value || null,
    } : null,
    upnp: document.getElementById('s-upnp').checked ? true : null,
    log_level: document.getElementById('s-log-level').value || null,
    show_stats: document.getElementById('s-show-stats').checked ? true : null,
    seeder_threads: parseInt(document.getElementById('s-seeder-threads').value) || null,
    web_threads: parseInt(document.getElementById('s-web-threads').value) || null,
    source_folder: document.getElementById('s-source-folder').value.trim() || null,
    letsencrypt_domain: document.getElementById('s-le-domain').value.trim() || null,
    letsencrypt_email: document.getElementById('s-le-email').value.trim() || null,
    letsencrypt_http_port: parseInt(document.getElementById('s-le-port').value) || null,
    web_login_rate_limit: parseInt(document.getElementById('s-login-rate-limit').value) || null,
  };
  try {
    const r = await apiFetch('/api/config', {
      method: 'PUT',
      body: JSON.stringify(cfg),
    });
    if (r.ok) {
      $('#settings-modal').modal('hide');
      document.getElementById('status-text').textContent = 'Settings saved — seeders reloading…';
    } else {
      alert('Failed to save settings: ' + await r.text());
    }
  } catch(e) {
    if (e.message !== 'Unauthorized') alert('Error: ' + e.message);
  }
}

let pending2FASecret = null;

async function open2FASetup() {
  try {
    const r = await apiFetch('/api/2fa/setup', {method: 'POST'});
    const d = await r.json();
    pending2FASecret = d.secret;
    document.getElementById('twofa-secret-text').textContent = d.secret;
    document.getElementById('twofa-confirm-code').value = '';
    document.getElementById('twofa-error').style.display = 'none';
    
    // Generate QR code with proper settings for scannability
    const qrEl = document.getElementById('twofa-qr-canvas');
    qrEl.innerHTML = '';
    
    // Use qrcodejs2 with optimal settings for scanning
    new QRCode(qrEl, {
      text: d.otpauth_uri,
      width: 256,
      height: 256,
      colorDark: "#000000",
      colorLight: "#ffffff",
      correctLevel: QRCode.CorrectLevel.H,  // High error correction for better scannability
      margin: 2,  // Proper quiet zone
    });
    
    $('#twofa-modal').modal('show');
  } catch(e) {
    if (e.message !== 'Unauthorized') alert('Error: ' + e.message);
  }
}

async function confirm2FA() {
  const code = document.getElementById('twofa-confirm-code').value.trim();
  try {
    const r = await apiFetch('/api/2fa/confirm', {
      method: 'POST',
      body: JSON.stringify({secret: pending2FASecret, code}),
    });
    if (r.ok) {
      $('#twofa-modal').modal('hide');
      await openSettings();
    } else {
      const e = await r.json().catch(() => ({error: 'Unknown error'}));
      document.getElementById('twofa-error').textContent = e.error || 'Invalid code';
      document.getElementById('twofa-error').style.display = '';
    }
  } catch(e) {
    if (e.message !== 'Unauthorized') alert('Error: ' + e.message);
  }
}

async function disable2FA() {
  if (!confirm('Disable two-factor authentication? You will only need your password to log in.')) return;
  try {
    await apiFetch('/api/2fa/disable', {method: 'DELETE'});
    await openSettings();
  } catch(e) {
    if (e.message !== 'Unauthorized') alert('Error: ' + e.message);
  }
}

setInterval(loadStats, 5000);

$(document).ready(function() {
  $('.ui.checkbox').checkbox();
  $('.tabular.menu .item').tab();
  const savedTheme = localStorage.getItem('seeder_theme') || 'auto';
  setTheme(savedTheme);
});

let browserTarget = null;
let browserMode = 'any';
let browserPath = '/';

function openBrowser(inputId, mode) {
  browserTarget = inputId;
  browserMode = mode;
  const cur = document.getElementById(inputId).value.trim();
  let startPath = '';
  if (cur) {
    const slash = cur.lastIndexOf('/');
    startPath = slash > 0 ? cur.substring(0, slash) : '';
  }
  document.getElementById('browser-title').textContent =
    mode === 'file' ? 'Select File' : 'Select File or Folder';
  document.getElementById('browser-select-dir-btn').style.display =
    mode === 'file' ? 'none' : '';
  browserNavigate(startPath);
  $('#file-browser-modal').modal({
    onHidden: function() {
      if (['s-web-cert','s-web-key','s-source-folder'].includes(browserTarget)) {
        $('#settings-modal').modal('show');
      } else if (browserTarget === 'u-dest-folder') {
        $('#upload-modal').modal('show');
      }
    }
  }).modal('show');
}

function browserShowMkdir() {
  const row = document.getElementById('browser-mkdir-row');
  row.style.display = 'flex';
  const inp = document.getElementById('browser-mkdir-name');
  inp.value = '';
  inp.focus();
}

function browserHideMkdir() {
  document.getElementById('browser-mkdir-row').style.display = 'none';
}

async function browserMkdir() {
  const name = document.getElementById('browser-mkdir-name').value.trim();
  if (!name) return;
  const newPath = browserPath.replace(/\/$/, '') + '/' + name;
  try {
    const r = await apiFetch('/api/mkdir', {
      method: 'POST',
      body: JSON.stringify({ path: newPath }),
    });
    if (!r.ok) {
      const err = await r.json().catch(() => ({}));
      alert('Failed to create folder: ' + (err.error || 'Unknown error'));
      return;
    }
    browserHideMkdir();
    browserNavigate(newPath);
  } catch(e) {
    if (e.message !== 'Unauthorized') alert('Error: ' + e.message);
  }
}

async function browserNavigate(path) {
  browserHideMkdir();
  browserPath = path;
  document.getElementById('browser-list').innerHTML =
    '<div style="padding:16px;color:#999;text-align:center">Loading…</div>';
  try {
    const r = await apiFetch('/api/browse?path=' + encodeURIComponent(path));
    if (!r.ok) {
      document.getElementById('browser-list').innerHTML =
        '<div style="padding:12px;color:#c00">Error: ' + escHtml(await r.text()) + '</div>';
      return;
    }
    const data = await r.json();
    browserPath = data.path;
    const parts = (data.path === '/' ? [] : data.path.replace(/\/$/, '').split('/').filter(Boolean));
    let bc = '<div class="ui small breadcrumb">';
    bc += '<a class="section" href="#" onclick="browserNavigate(\'\/\');return false;">/</a>';
    let built = '';
    for (const p of parts) {
      built += '/' + p;
      const bp = built;
      bc += '<i class="right angle icon divider"></i>';
      bc += `<a class="section" href="#" onclick="browserNavigate(${JSON.stringify(bp)});return false;">${escHtml(p)}</a>`;
    }
    bc += '</div>';
    document.getElementById('browser-breadcrumb').innerHTML = bc;
    const base = data.path === '/' ? '' : data.path.replace(/\/$/, '');
    let html = '';
    if (data.parent !== null) {
      html += `<div class="browser-item" data-path="${escAttr(data.parent)}" data-isdir="true"
                 style="padding:8px 12px;cursor:pointer;display:flex;align-items:center;gap:8px;border-bottom:1px solid #f0f0f0">
        <i class="level up alternate icon" style="color:#888;margin:0;flex-shrink:0"></i>
        <span style="color:#555">..</span>
      </div>`;
    }
    for (const e of data.entries) {
      const fp = base + '/' + e.name;
      if (e.is_dir) {
        html += `<div class="browser-item" data-path="${escAttr(fp)}" data-isdir="true"
                   style="padding:8px 12px;cursor:pointer;display:flex;align-items:center;gap:8px;border-bottom:1px solid #f0f0f0">
          <i class="folder icon" style="color:#f0c040;margin:0;flex-shrink:0"></i>
          <span>${escHtml(e.name)}</span>
        </div>`;
      } else {
        const sel = browserMode !== 'dir';
        html += `<div class="browser-item" data-path="${escAttr(fp)}" data-isdir="false" data-selectable="${sel}"
                   style="padding:8px 12px;${sel?'cursor:pointer':'opacity:0.45'};display:flex;align-items:center;gap:8px;border-bottom:1px solid #f0f0f0">
          <i class="file outline icon" style="color:#888;margin:0;flex-shrink:0"></i>
          <span>${escHtml(e.name)}</span>
          <span style="color:#aaa;font-size:0.82em;margin-left:auto;flex-shrink:0">${fmtBytes(e.size)}</span>
        </div>`;
      }
    }
    if (!html) {
      html = '<div style="padding:16px;color:#999;text-align:center">Empty folder</div>';
    }
    document.getElementById('browser-list').innerHTML = html;
  } catch(e) {
    if (e.message !== 'Unauthorized') {
      document.getElementById('browser-list').innerHTML =
        '<div style="padding:12px;color:#c00">Error: ' + escHtml(e.message) + '</div>';
    }
  }
}

function browserSelectFile(path) {
  document.getElementById(browserTarget).value = path;
  $('#file-browser-modal').modal('hide');
}

function browserSelectFolder() {
  document.getElementById(browserTarget).value = browserPath;
  $('#file-browser-modal').modal('hide');
}

$(document).on('click', '.browser-item', function() {
  const path = this.dataset.path;
  const isDir = this.dataset.isdir === 'true';
  const sel = this.dataset.selectable !== 'false';
  if (isDir) {
    browserNavigate(path);
  } else if (sel) {
    browserSelectFile(path);
  }
});

let uploadFiles = [];
let uploadCancelled = false;
let uploadIncludeFolder = true;
const UPLOAD_CHUNK_SIZE = 4 * 1024 * 1024;

function effectiveRelPath(relPath) {
  if (uploadIncludeFolder) return relPath;
  const slash = relPath.indexOf('/');
  return slash >= 0 ? relPath.slice(slash + 1) : relPath;
}

function openUploadModal() {
  uploadFiles = [];
  uploadCancelled = false;
  uploadIncludeFolder = true;
  document.getElementById('u-include-folder').checked = true;
  document.getElementById('u-file-input').value = '';
  document.getElementById('u-folder-input').value = '';
  document.getElementById('u-file-summary').textContent = '';
  showUploadPhase('select');
  renderUploadFileList();
  $('#upload-modal').modal('show');
}

function showUploadPhase(phase) {
  document.getElementById('u-select-section').style.display = phase === 'select' ? '' : 'none';
  document.getElementById('u-progress-section').style.display = phase === 'upload' ? '' : 'none';
  document.getElementById('u-btn-upload').style.display = phase === 'select' ? '' : 'none';
  document.getElementById('u-btn-close').textContent = phase === 'select' ? 'Cancel' : 'Close';
}

function onUploadFilesSelected(input) {
  for (const file of input.files) {
    const relPath = file.webkitRelativePath || file.name;
    if (!uploadFiles.some(f => f.relPath === relPath)) {
      uploadFiles.push({ file, relPath });
    }
  }
  renderUploadFileList();
}

function uploadRemoveFile(idx) {
  uploadFiles.splice(idx, 1);
  renderUploadFileList();
}

function renderUploadFileList() {
  const list = document.getElementById('u-file-list');
  const summary = document.getElementById('u-file-summary');
  if (uploadFiles.length === 0) {
    list.innerHTML = '<div style="padding:16px;color:#999;text-align:center">No files selected</div>';
    summary.textContent = '';
    return;
  }
  const totalBytes = uploadFiles.reduce((s, f) => s + f.file.size, 0);
  let html = '';
  for (let i = 0; i < uploadFiles.length; i++) {
    const { file, relPath } = uploadFiles[i];
    const displayPath = effectiveRelPath(relPath);
    html += `<div class="upload-file-item" style="display:flex;align-items:center;gap:8px;padding:6px 10px;border-bottom:1px solid #f0f0f0">
      <i class="file outline icon" style="color:#888;margin:0;flex-shrink:0"></i>
      <span style="flex:1;word-break:break-all;font-size:0.88em">${escHtml(displayPath)}</span>
      <span style="color:#aaa;font-size:0.82em;flex-shrink:0;margin-right:4px">${fmtBytes(file.size)}</span>
      <button class="ui mini icon button" type="button" onclick="uploadRemoveFile(${i})" style="flex-shrink:0;padding:4px 6px"><i class="times icon" style="margin:0"></i></button>
    </div>`;
  }
  list.innerHTML = html;
  summary.textContent = `${uploadFiles.length} file${uploadFiles.length !== 1 ? 's' : ''} — ${fmtBytes(totalBytes)} total`;
}

async function sha256Hex(buffer) {
  const hash = await crypto.subtle.digest('SHA-256', buffer);
  return Array.from(new Uint8Array(hash)).map(b => b.toString(16).padStart(2, '0')).join('');
}

async function startUpload() {
  const destBase = document.getElementById('u-dest-folder').value.trim();
  if (!destBase) { alert('Please select a destination folder on the server.'); return; }
  if (uploadFiles.length === 0) { alert('No files selected.'); return; }
  uploadCancelled = false;
  showUploadPhase('upload');
  let html = '';
  for (let i = 0; i < uploadFiles.length; i++) {
    html += `<div style="margin-bottom:12px">
      <div style="display:flex;justify-content:space-between;align-items:baseline;font-size:0.87em;margin-bottom:3px">
        <span style="word-break:break-all;flex:1;margin-right:8px">${escHtml(uploadFiles[i].relPath)}</span>
        <span id="u-lbl-${i}" style="flex-shrink:0;color:#888">Waiting…</span>
      </div>
      <div style="background:#e0e0e0;border-radius:4px;height:6px;overflow:hidden">
        <div id="u-bar-${i}" style="height:100%;width:0%;background:#2185d0;transition:width 0.15s;border-radius:4px"></div>
      </div>
    </div>`;
  }
  document.getElementById('u-progress-list').innerHTML = html;
  document.getElementById('u-overall-label').textContent = `0 / ${uploadFiles.length} files done`;
  document.getElementById('u-overall-bar').style.cssText = 'height:100%;width:0%;background:#2185d0;transition:width 0.25s;border-radius:4px';
  let done = 0;
  for (let i = 0; i < uploadFiles.length; i++) {
    const lbl = document.getElementById(`u-lbl-${i}`);
    const bar = document.getElementById(`u-bar-${i}`);
    if (uploadCancelled) {
      lbl.textContent = 'Skipped';
      lbl.style.color = '#aaa';
      done++;
      continue;
    }
    lbl.textContent = '0%';
    lbl.style.color = '';
    try {
      await uploadSingleFile(uploadFiles[i].file, effectiveRelPath(uploadFiles[i].relPath), destBase,
        pct => {
          bar.style.width = (pct * 100).toFixed(1) + '%';
          lbl.textContent = (pct * 100).toFixed(0) + '%';
        },
        pct => {
          bar.style.background = '#f2711c';
          bar.style.width = pct + '%';
          lbl.textContent = 'Verifying ' + pct + '%';
          lbl.style.color = '#f2711c';
        });
      bar.style.width = '100%';
      bar.style.background = '#21ba45';
      lbl.textContent = '✓ Done';
      lbl.style.color = '#21ba45';
    } catch (e) {
      if (e.message === 'Cancelled') {
        lbl.textContent = 'Cancelled';
        lbl.style.color = '#aaa';
      } else {
        bar.style.background = '#db2828';
        lbl.textContent = '✗ ' + e.message;
        lbl.style.color = '#db2828';
      }
    }
    done++;
    document.getElementById('u-overall-label').textContent = `${done} / ${uploadFiles.length} files done`;
    const overallBar = document.getElementById('u-overall-bar');
    overallBar.style.width = ((done / uploadFiles.length) * 100) + '%';
    if (done === uploadFiles.length && !uploadCancelled) {
      overallBar.style.background = '#21ba45';
    }
  }
  if (uploadCancelled) {
    document.getElementById('u-overall-label').textContent = 'Upload stopped.';
  }
}

async function uploadSingleFile(file, relPath, destBase, onProgress, onHashProgress) {
  const dest = destBase.replace(/\/+$/, '') + '/' + relPath;
  const totalChunks = Math.max(1, Math.ceil(file.size / UPLOAD_CHUNK_SIZE));
  const fileSha256 = await sha256Hex(await file.arrayBuffer());
  const initR = await apiFetch('/api/file-upload/init', {
    method: 'POST',
    body: JSON.stringify({ dest, size: file.size, chunks: totalChunks, chunk_size: UPLOAD_CHUNK_SIZE, file_sha256: fileSha256 }),
  });
  if (!initR.ok) throw new Error(await initR.text());
  const { upload_id } = await initR.json();
  try {
    for (let i = 0; i < totalChunks; i++) {
      if (uploadCancelled) {
        await apiFetch(`/api/file-upload/${encodeURIComponent(upload_id)}`, { method: 'DELETE' }).catch(() => {});
        throw new Error('Cancelled');
      }
      const start = i * UPLOAD_CHUNK_SIZE;
      const chunkBuf = await file.slice(start, Math.min(start + UPLOAD_CHUNK_SIZE, file.size)).arrayBuffer();
      const sha256 = await sha256Hex(chunkBuf);
      let lastErr = null;
      for (let attempt = 0; attempt < 3; attempt++) {
        const r = await apiFetchBinary(
          `/api/file-upload/chunk?id=${encodeURIComponent(upload_id)}&n=${i}&sha256=${sha256}`,
          chunkBuf,
        );
        if (r.ok) { lastErr = null; break; }
        lastErr = await r.text();
      }
      if (lastErr) throw new Error(`Chunk ${i}: ${lastErr}`);
      onProgress((i + 1) / totalChunks);
    }
  } catch (e) {
    await apiFetch(`/api/file-upload/${encodeURIComponent(upload_id)}`, { method: 'DELETE' }).catch(() => {});
    throw e;
  }
  onHashProgress(0);
  let polling = true;
  (async () => {
    while (polling) {
      await new Promise(r => setTimeout(r, 400));
      if (!polling) break;
      try {
        const pr = await apiFetch(`/api/file-upload/${encodeURIComponent(upload_id)}/hash-progress`);
        if (pr.ok) {
          const { percent } = await pr.json();
          onHashProgress(percent);
        }
      } catch (_) {}
    }
  })();
  try {
    const finR = await apiFetch('/api/file-upload/finalize', {
      method: 'POST',
      body: JSON.stringify({ upload_id }),
    });
    if (!finR.ok) {
      const err = await finR.json().catch(() => ({}));
      throw new Error(err.error || 'Finalize failed');
    }
    return finR.json();
  } finally {
    polling = false;
  }
}

async function apiFetchBinary(url, body) {
  const headers = { 'Content-Type': 'application/octet-stream' };
  if (authToken) headers['Authorization'] = 'Bearer ' + authToken;
  const r = await fetch(url, { method: 'POST', headers, body });
  if (r.status === 401) {
    authToken = null;
    localStorage.removeItem('seeder_token');
    disconnectWs();
    showLogin();
    throw new Error('Unauthorized');
  }
  return r;
}

function cancelUpload() {
  uploadCancelled = true;
  document.getElementById('u-overall-label').textContent = 'Stopping after current chunk…';
}

async function uploadTorrentFile() {
  const fileInput = document.getElementById('f-torrent-upload');
  const file = fileInput.files[0];
  if (!file) return;
  try {
    const arrayBuffer = await file.arrayBuffer();
    const headers = {};
    if (authToken) headers['Authorization'] = 'Bearer ' + authToken;
    headers['Content-Type'] = 'application/octet-stream';
    const r = await fetch('/api/upload-torrent?name=' + encodeURIComponent(file.name), {
      method: 'POST',
      headers,
      body: arrayBuffer,
    });
    if (r.status === 401) {
      authToken = null;
      localStorage.removeItem('seeder_token');
      showLogin();
      return;
    }
    if (r.ok) {
      const result = await r.json();
      document.getElementById('f-torrent-file').value = result.path;
    } else {
      alert('Upload failed: ' + await r.text());
    }
  } catch(e) {
    alert('Upload error: ' + e.message);
  }
  fileInput.value = '';
}

function showError(msg) {
  document.getElementById('error-modal-msg').textContent = msg;
  $('#error-modal').modal('show');
}

async function batchAdd() {
  try {
    const r = await apiFetch('/api/batch-add', { method: 'POST' });
    if (r.ok) {
      const result = await r.json();
      if (result.added === 0) {
        const msg = result.skipped > 0
          ? `No new items found — ${result.skipped} already tracked.`
          : 'Source folder is empty or has no new items.';
        showError(msg);
      } else {
        document.getElementById('status-text').textContent =
          `Batch add: ${result.added} new torrent(s) added, ${result.skipped} already tracked — reloading…`;
        setTimeout(loadTorrents, 2000);
      }
    } else {
      let msg;
      try { msg = (await r.json()).error; } catch { msg = await r.text(); }
      showError('Batch add failed: ' + msg);
    }
  } catch(e) {
    if (e.message !== 'Unauthorized') showError('Error: ' + e.message);
  }
}
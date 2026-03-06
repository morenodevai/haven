'use strict';

const state = {
    currentSection: 'overview',
    token: null,
    ws: null,
    wsRetries: 0,
    stats: {},
    users: [],
    channels: [],
};

const MAX_WS_RETRIES = 5;
const WS_RETRY_DELAY_MS = 3000;

// ── Utilities ────────────────────────────────────────────────────────────

function formatBytes(bytes) {
    if (bytes === 0) return '0 B';
    const units = ['B', 'KB', 'MB', 'GB', 'TB'];
    const i = Math.floor(Math.log(bytes) / Math.log(1024));
    return (bytes / Math.pow(1024, i)).toFixed(1) + ' ' + units[i];
}

function formatUptime(secs) {
    const d = Math.floor(secs / 86400);
    const h = Math.floor((secs % 86400) / 3600);
    const m = Math.floor((secs % 3600) / 60);
    const s = secs % 60;
    if (d > 0) return d + 'd ' + h + 'h ' + m + 'm';
    return h + 'h ' + m + 'm ' + s + 's';
}

function truncateId(id) {
    if (!id) return '';
    return id.substring(0, 8) + '...';
}

function escapeAttr(str) {
    return String(str).replace(/&/g, '&amp;').replace(/"/g, '&quot;')
        .replace(/'/g, '&#39;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
}

function escapeHtml(str) {
    const div = document.createElement('div');
    div.textContent = str;
    return div.innerHTML;
}

// ── API ──────────────────────────────────────────────────────────────────

async function api(method, path, body) {
    const opts = {
        method,
        headers: {
            'Authorization': 'Bearer ' + state.token,
            'Content-Type': 'application/json',
        },
    };
    if (body !== undefined) opts.body = JSON.stringify(body);

    try {
        const res = await fetch(path, opts);
        if (res.status === 401 || res.status === 403) { logout(); return null; }
        if (!res.ok) {
            console.error(method, path, 'failed:', res.status);
            return null;
        }
        if (res.status === 204) return {};
        return res.json();
    } catch (e) {
        console.error(method, path, 'error:', e);
        return null;
    }
}

// ── Auth ─────────────────────────────────────────────────────────────────

function logout() {
    sessionStorage.removeItem('admin_token');
    state.token = null;
    if (state.ws) { state.ws.close(); state.ws = null; }
    document.getElementById('dashboard').hidden = true;
    document.getElementById('login-page').hidden = false;
    document.getElementById('login-secret').value = '';
}

document.getElementById('login-form').addEventListener('submit', async (e) => {
    e.preventDefault();
    const secret = document.getElementById('login-secret').value.trim();
    const errEl = document.getElementById('login-error');
    errEl.hidden = true;

    try {
        const res = await fetch('/admin/login', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ secret }),
        });
        if (!res.ok) {
            errEl.textContent = 'Invalid secret';
            errEl.hidden = false;
            return;
        }
        const data = await res.json();
        sessionStorage.setItem('admin_token', data.token);
        state.token = data.token;
        enterDashboard();
    } catch (err) {
        errEl.textContent = 'Connection error';
        errEl.hidden = false;
    }
});

document.getElementById('logout-btn').addEventListener('click', logout);

// ── Event delegation ─────────────────────────────────────────────────────
// All button actions use data-action attributes instead of inline onclick.

document.querySelector('.content').addEventListener('click', (e) => {
    const btn = e.target.closest('[data-action]');
    if (!btn) return;

    const action = btn.dataset.action;
    const id = btn.dataset.id;
    const name = btn.dataset.name;

    switch (action) {
        case 'kick-user':
            api('POST', '/admin/kick/' + id).then(fetchUsers);
            break;
        case 'delete-user':
            showConfirm('Delete user "' + name + '"? This cannot be undone.', async () => {
                await api('DELETE', '/admin/users/' + id);
                fetchUsers();
            });
            break;
        case 'delete-channel':
            showConfirm('Delete channel "' + name + '" and all its messages? This cannot be undone.', async () => {
                await api('DELETE', '/admin/channels/' + id);
                fetchChannels();
            });
            break;
        case 'delete-transfer':
            api('DELETE', '/admin/transfers/' + id).then(fetchTransfers);
            break;
        case 'clear-offer':
            api('DELETE', '/admin/offers/' + id).then(fetchOffers);
            break;
    }
});

// ── Dashboard entry ──────────────────────────────────────────────────────

function enterDashboard() {
    document.getElementById('login-page').hidden = true;
    document.getElementById('dashboard').hidden = false;
    showSection('overview');
    connectWs();
}

// ── Navigation ───────────────────────────────────────────────────────────

document.querySelectorAll('.nav-item[data-section]').forEach(el => {
    el.addEventListener('click', () => showSection(el.dataset.section));
});

function showSection(name) {
    state.currentSection = name;
    document.querySelectorAll('.section').forEach(s => s.hidden = true);
    document.getElementById('section-' + name).hidden = false;
    document.querySelectorAll('.nav-item').forEach(n => n.classList.remove('active'));
    const navEl = document.querySelector('[data-section="' + name + '"]');
    if (navEl) navEl.classList.add('active');

    switch (name) {
        case 'overview': fetchStats(); break;
        case 'users': fetchUsers(); break;
        case 'channels': fetchChannels(); break;
        case 'messages': fetchChannelFilter(); fetchMessages(); break;
        case 'voice': fetchVoice(); break;
        case 'transfers': fetchTransfers(); break;
        case 'offers': fetchOffers(); break;
        case 'config': fetchConfig(); break;
    }
}

// ── Overview ─────────────────────────────────────────────────────────────

async function fetchStats() {
    const data = await api('GET', '/admin/stats');
    if (data) { state.stats = data; renderOverview(); }
}

function renderOverview() {
    const s = state.stats;
    const container = document.getElementById('overview-cards');
    container.innerHTML = '';
    const cards = [
        { label: 'Uptime', value: formatUptime(s.uptime_secs || 0) },
        { label: 'Online Users', value: s.online_count || 0 },
        { label: 'Total Users', value: s.total_users || 0 },
        { label: 'Messages', value: s.total_messages || 0 },
        { label: 'Channels', value: s.total_channels || 0 },
        { label: 'File Offers', value: s.pending_file_offers || 0 },
        { label: 'Folder Offers', value: s.pending_folder_offers || 0 },
    ];
    cards.forEach(c => {
        const card = document.createElement('div');
        card.className = 'stat-card';
        card.innerHTML = '<div class="value">' + escapeHtml(String(c.value)) + '</div>'
            + '<div class="label">' + escapeHtml(c.label) + '</div>';
        container.appendChild(card);
    });
    document.getElementById('header-uptime').textContent = 'Uptime: ' + formatUptime(s.uptime_secs || 0);
}

// ── Users ────────────────────────────────────────────────────────────────

async function fetchUsers() {
    const data = await api('GET', '/admin/users');
    if (data) { state.users = data.users; renderUsers(); }
}

function renderUsers() {
    const container = document.getElementById('users-content');
    if (!state.users.length) {
        container.innerHTML = '<p class="empty-state">No users</p>';
        return;
    }
    let html = '<table><thead><tr><th>Status</th><th>Username</th><th>ID</th><th>Created</th><th>Actions</th></tr></thead><tbody>';
    state.users.forEach(u => {
        const dot = u.online ? 'online' : 'offline';
        html += '<tr data-user-id="' + escapeAttr(u.id) + '">'
            + '<td><span class="status-dot ' + dot + '"></span></td>'
            + '<td>' + escapeHtml(u.username) + '</td>'
            + '<td title="' + escapeAttr(u.id) + '">' + escapeHtml(truncateId(u.id)) + '</td>'
            + '<td>' + escapeHtml(u.created_at) + '</td>'
            + '<td>';
        if (u.online) {
            html += '<button class="danger small" data-action="kick-user" data-id="' + escapeAttr(u.id) + '">Kick</button> ';
        }
        html += '<button class="danger small" data-action="delete-user" data-id="' + escapeAttr(u.id) + '" data-name="' + escapeAttr(u.username) + '">Delete</button>';
        html += '</td></tr>';
    });
    html += '</tbody></table>';
    container.innerHTML = html;
}

// ── Channels ─────────────────────────────────────────────────────────────

async function fetchChannels() {
    const data = await api('GET', '/admin/channels');
    if (data) { state.channels = data.channels; renderChannels(); }
}

function renderChannels() {
    const container = document.getElementById('channels-content');
    if (!state.channels.length) {
        container.innerHTML = '<p class="empty-state">No channels</p>';
        return;
    }
    let html = '<table><thead><tr><th>Name</th><th>ID</th><th>Messages</th><th>Actions</th></tr></thead><tbody>';
    state.channels.forEach(c => {
        html += '<tr>'
            + '<td>' + escapeHtml(c.name) + '</td>'
            + '<td title="' + escapeAttr(c.id) + '">' + escapeHtml(truncateId(c.id)) + '</td>'
            + '<td>' + c.message_count + '</td>'
            + '<td><button class="danger small" data-action="delete-channel" data-id="' + escapeAttr(c.id) + '" data-name="' + escapeAttr(c.name) + '">Delete</button></td>'
            + '</tr>';
    });
    html += '</tbody></table>';
    container.innerHTML = html;
}

document.getElementById('create-channel-form').addEventListener('submit', async (e) => {
    e.preventDefault();
    const input = document.getElementById('new-channel-name');
    const name = input.value.trim();
    if (!name) return;
    await api('POST', '/admin/channels', { name });
    input.value = '';
    fetchChannels();
});

// ── Messages ─────────────────────────────────────────────────────────────

async function fetchChannelFilter() {
    const data = await api('GET', '/admin/channels');
    if (!data) return;
    const select = document.getElementById('msg-channel-filter');
    while (select.options.length > 1) select.remove(1);
    data.channels.forEach(c => {
        const opt = document.createElement('option');
        opt.value = c.id;
        opt.textContent = c.name;
        select.appendChild(opt);
    });
}

document.getElementById('msg-channel-filter').addEventListener('change', fetchMessages);

async function fetchMessages() {
    const channelId = document.getElementById('msg-channel-filter').value;
    let url = '/admin/messages?limit=100';
    if (channelId) url += '&channel_id=' + encodeURIComponent(channelId);
    const data = await api('GET', url);
    if (data) renderMessages(data.messages);
}

function renderMessages(messages) {
    const container = document.getElementById('messages-content');
    if (!messages || !messages.length) {
        container.innerHTML = '<p class="empty-state">No messages</p>';
        return;
    }
    let html = '<table><thead><tr><th>Time</th><th>Channel</th><th>Author</th><th>Size</th></tr></thead><tbody>';
    messages.forEach(m => {
        html += '<tr>'
            + '<td>' + escapeHtml(m.created_at) + '</td>'
            + '<td title="' + escapeAttr(m.channel_id) + '">' + escapeHtml(truncateId(m.channel_id)) + '</td>'
            + '<td>' + escapeHtml(m.author_username) + '</td>'
            + '<td>' + formatBytes(m.byte_length) + '</td>'
            + '</tr>';
    });
    html += '</tbody></table>';
    container.innerHTML = html;
}

// ── Voice ────────────────────────────────────────────────────────────────

async function fetchVoice() {
    const data = await api('GET', '/admin/voice');
    if (data) renderVoice(data.channels);
}

function renderVoice(channels) {
    const container = document.getElementById('voice-content');
    if (!channels || !channels.length) {
        container.innerHTML = '<p class="empty-state">No active voice channels</p>';
        return;
    }
    let html = '';
    channels.forEach(ch => {
        html += '<div class="voice-card">'
            + '<h3>Channel ' + escapeHtml(truncateId(ch.channel_id)) + '</h3>';
        ch.participants.forEach(p => {
            html += '<div class="voice-participant">'
                + '<span class="status-dot online"></span>'
                + escapeHtml(p.username);
            if (p.self_mute) html += ' <span class="mute-badge">MUTE</span>';
            if (p.self_deaf) html += ' <span class="deaf-badge">DEAF</span>';
            html += '</div>';
        });
        html += '</div>';
    });
    container.innerHTML = html;
}

// ── Transfers ────────────────────────────────────────────────────────────

async function fetchTransfers() {
    const data = await api('GET', '/admin/transfers');
    if (data) renderTransfers(data.transfers);
}

function renderTransfers(transfers) {
    const container = document.getElementById('transfers-content');
    if (!transfers || !transfers.length) {
        container.innerHTML = '<p class="empty-state">No transfers</p>';
        return;
    }
    let html = '<table><thead><tr><th>ID</th><th>Uploader</th><th>Size</th><th>Received</th><th>Status</th><th>Created</th><th>Actions</th></tr></thead><tbody>';
    transfers.forEach(t => {
        html += '<tr>'
            + '<td title="' + escapeAttr(t.id) + '">' + escapeHtml(truncateId(t.id)) + '</td>'
            + '<td title="' + escapeAttr(t.uploader_id) + '">' + escapeHtml(truncateId(t.uploader_id)) + '</td>'
            + '<td>' + formatBytes(t.file_size) + '</td>'
            + '<td>' + formatBytes(t.bytes_received) + '</td>'
            + '<td>' + escapeHtml(t.status) + '</td>'
            + '<td>' + escapeHtml(t.created_at) + '</td>'
            + '<td><button class="danger small" data-action="delete-transfer" data-id="' + escapeAttr(t.id) + '">Delete</button></td>'
            + '</tr>';
    });
    html += '</tbody></table>';
    container.innerHTML = html;
}

// ── Offers ───────────────────────────────────────────────────────────────

async function fetchOffers() {
    const data = await api('GET', '/admin/offers');
    if (data) renderOffers(data);
}

function renderOffers(data) {
    const container = document.getElementById('offers-content');
    let html = '';

    html += '<h3>File Offers</h3>';
    if (!data.file_offers || !data.file_offers.length) {
        html += '<p class="empty-state">No file offers</p>';
    } else {
        html += '<table><thead><tr><th>Transfer ID</th><th>From</th><th>To</th><th>Filename</th><th>Size</th><th>Status</th><th>Actions</th></tr></thead><tbody>';
        data.file_offers.forEach(o => {
            html += '<tr>'
                + '<td title="' + escapeAttr(o.transfer_id) + '">' + escapeHtml(truncateId(o.transfer_id)) + '</td>'
                + '<td title="' + escapeAttr(o.from_user_id) + '">' + escapeHtml(truncateId(o.from_user_id)) + '</td>'
                + '<td title="' + escapeAttr(o.to_user_id) + '">' + escapeHtml(truncateId(o.to_user_id)) + '</td>'
                + '<td>' + escapeHtml(o.filename) + '</td>'
                + '<td>' + formatBytes(o.file_size) + '</td>'
                + '<td>' + escapeHtml(o.status) + '</td>'
                + '<td><button class="danger small" data-action="clear-offer" data-id="' + escapeAttr(o.transfer_id) + '">Clear</button></td>'
                + '</tr>';
        });
        html += '</tbody></table>';
    }

    html += '<h3 style="margin-top:24px">Folder Offers</h3>';
    if (!data.folder_offers || !data.folder_offers.length) {
        html += '<p class="empty-state">No folder offers</p>';
    } else {
        html += '<table><thead><tr><th>Folder ID</th><th>From</th><th>To</th><th>Name</th><th>Size</th><th>Files</th><th>Status</th></tr></thead><tbody>';
        data.folder_offers.forEach(f => {
            html += '<tr>'
                + '<td title="' + escapeAttr(f.folder_id) + '">' + escapeHtml(truncateId(f.folder_id)) + '</td>'
                + '<td title="' + escapeAttr(f.from_user_id) + '">' + escapeHtml(truncateId(f.from_user_id)) + '</td>'
                + '<td title="' + escapeAttr(f.to_user_id) + '">' + escapeHtml(truncateId(f.to_user_id)) + '</td>'
                + '<td>' + escapeHtml(f.folder_name) + '</td>'
                + '<td>' + formatBytes(f.total_size) + '</td>'
                + '<td>' + f.file_count + '</td>'
                + '<td>' + escapeHtml(f.status) + '</td>'
                + '</tr>';
        });
        html += '</tbody></table>';
    }

    container.innerHTML = html;
}

// ── Config ───────────────────────────────────────────────────────────────

async function fetchConfig() {
    const data = await api('GET', '/admin/config');
    if (!data) return;
    const container = document.getElementById('config-content');
    let html = '<table class="config-table"><tbody>';
    Object.entries(data).forEach(([key, val]) => {
        html += '<tr><td>' + escapeHtml(key) + '</td><td>' + escapeHtml(String(val)) + '</td></tr>';
    });
    html += '</tbody></table>';
    container.innerHTML = html;
}

// ── WebSocket ────────────────────────────────────────────────────────────

function connectWs() {
    if (state.ws) { state.ws.close(); state.ws = null; }
    if (!state.token) return;

    const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
    const url = proto + '//' + location.host + '/admin/ws?token=' + encodeURIComponent(state.token);
    state.ws = new WebSocket(url);

    state.ws.onopen = () => {
        state.wsRetries = 0;
        updateStatusBar('Connected');
    };

    state.ws.onmessage = (e) => {
        try {
            const msg = JSON.parse(e.data);
            handleWsMessage(msg);
        } catch (err) {
            console.error('WS parse error:', err);
        }
    };

    state.ws.onclose = () => {
        updateStatusBar('Disconnected');
        if (state.token && state.wsRetries < MAX_WS_RETRIES) {
            state.wsRetries++;
            setTimeout(connectWs, WS_RETRY_DELAY_MS);
        } else if (state.wsRetries >= MAX_WS_RETRIES) {
            updateStatusBar('Connection lost -- refresh page');
        }
    };

    state.ws.onerror = () => {};
}

function handleWsMessage(msg) {
    switch (msg.type) {
        case 'user_online':
        case 'user_offline':
            onPresenceUpdate(msg.data, msg.type === 'user_online');
            break;
        case 'voice_update':
            if (state.currentSection === 'voice') fetchVoice();
            break;
        case 'new_message':
            if (state.currentSection === 'messages') fetchMessages();
            break;
        case 'stats_update':
            state.stats = msg.data;
            if (state.currentSection === 'overview') renderOverview();
            document.getElementById('header-uptime').textContent = 'Uptime: ' + formatUptime(msg.data.uptime_secs || 0);
            updateStatusBar();
            break;
        case 'file_offer':
            if (state.currentSection === 'offers') fetchOffers();
            break;
    }
}

function onPresenceUpdate(data, online) {
    const user = state.users.find(u => u.id === data.user_id);
    if (user) user.online = online;

    if (state.currentSection === 'users') {
        fetchUsers();
    }
}

function updateStatusBar(connText) {
    if (connText) {
        document.getElementById('status-text').textContent = connText;
    }
    const online = state.stats.online_count;
    if (online !== undefined) {
        document.getElementById('status-online').textContent = online + ' user' + (online !== 1 ? 's' : '') + ' online';
    }
}

// ── Confirm dialog ───────────────────────────────────────────────────────

function showConfirm(message, onConfirm) {
    const overlay = document.createElement('div');
    overlay.className = 'confirm-overlay';

    const box = document.createElement('div');
    box.className = 'confirm-box';

    const p = document.createElement('p');
    p.textContent = message;
    box.appendChild(p);

    const actions = document.createElement('div');
    actions.className = 'actions';

    const cancelBtn = document.createElement('button');
    cancelBtn.className = 'primary';
    cancelBtn.textContent = 'Cancel';
    cancelBtn.addEventListener('click', () => overlay.remove());

    const confirmBtn = document.createElement('button');
    confirmBtn.className = 'danger';
    confirmBtn.textContent = 'Confirm';
    confirmBtn.addEventListener('click', () => { overlay.remove(); onConfirm(); });

    actions.appendChild(cancelBtn);
    actions.appendChild(confirmBtn);
    box.appendChild(actions);
    overlay.appendChild(box);

    overlay.addEventListener('click', (e) => {
        if (e.target === overlay) overlay.remove();
    });

    document.body.appendChild(overlay);
}

// ── Init ─────────────────────────────────────────────────────────────────

(function init() {
    const token = sessionStorage.getItem('admin_token');
    if (token) {
        state.token = token;
        enterDashboard();
    }
})();

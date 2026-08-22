// 移动端连接页逻辑：配对（扫码/手动）、服务器列表管理、跳转主界面。
// 仅依赖 window.__TAURI__（withGlobalTauri 注入），无构建依赖。
;(function () {
  'use strict'

  var zh = String(navigator.language || '').toLowerCase().indexOf('zh') === 0
  var t = function (zhText, enText) {
    return zh ? zhText : enText
  }

  var invoke = window.__TAURI__ && window.__TAURI__.core && window.__TAURI__.core.invoke

  var el = function (id) {
    return document.getElementById(id)
  }

  var strings = {
    subtitle: t(
      '可以直接在手机上使用，也可以连接到你的桌面 Pisper 获得完整能力。',
      'Use Pisper directly on this phone, or connect to your desktop Pisper for the full experience.',
    ),
    servers: t('已配对的桌面端', 'Paired desktops'),
    pairTitle: t('连接桌面端', 'Connect a desktop'),
    pairDesc: t(
      '远程模式由电脑运行完整 Runtime，会话与数据保存在电脑；优先局域网直连，不可达时自动尝试 P2P。',
      'Remote mode runs the full runtime on your computer and keeps data there. The app prefers LAN and falls back to P2P.',
    ),
    scan: t('扫码配对', 'Scan QR code to pair'),
    localTitle: t('本机运行', 'On-device'),
    localDesc: t(
      '不连接桌面端，直接在手机上运行受限 Runtime，通过 OpenAI 兼容 Provider 对话。',
      'Chat without a desktop: a constrained on-device runtime talks directly to an OpenAI-compatible provider.',
    ),
    enterLocal: t('进入本机模式', 'Open on-device mode'),
    url: t('桌面端地址', 'Desktop address'),
    code: t('配对码', 'Pairing code'),
    fp: t('证书指纹（桌面端设置页可见，可只填前 16 位）', 'Certificate fingerprint (see desktop settings; first 16 hex chars suffice)'),
    manualToggle: t('手动输入配对信息', 'Enter pairing details manually'),
    manual: t('完成配对', 'Pair'),
    manualHint: t(
      '桌面端入口：设置 → 远程访问。扫码会保存局域网与 P2P 地址；手动配对仅添加填写的 HTTPS 地址。',
      'On desktop: Settings → Remote access. QR pairing saves LAN and P2P endpoints; manual pairing adds only the HTTPS address entered here.',
    ),
    connect: t('连接', 'Connect'),
    forget: t('删除', 'Forget'),
    pairing: t('配对中…', 'Pairing…'),
    connecting: t('连接中…', 'Connecting…'),
    noTauri: t('原生桥不可用：请通过 Pisper App 打开本页。', 'Native bridge unavailable. Open this page from the Pisper app.'),
    scanFailed: t('扫码取消或失败', 'Scan cancelled or failed'),
    cameraDenied: t('相机权限被拒绝，请使用手动配对。', 'Camera permission denied. Use manual pairing instead.'),
    forgetConfirm: t('删除该桌面端？', 'Forget this desktop?'),
  }

  el('subtitle').textContent = strings.subtitle
  el('servers-title').textContent = strings.servers
  el('pair-title').textContent = strings.pairTitle
  el('pair-desc').textContent = strings.pairDesc
  el('scan-btn').textContent = strings.scan
  el('local-title').textContent = strings.localTitle
  el('local-desc').textContent = strings.localDesc
  el('local-btn').textContent = strings.enterLocal
  el('label-url').textContent = strings.url
  el('label-code').textContent = strings.code
  el('label-fp').textContent = strings.fp
  el('manual-toggle').textContent = strings.manualToggle
  el('manual-btn').textContent = strings.manual
  el('manual-hint').textContent = strings.manualHint

  // 手动配对输入区默认收纳，点击才展开子面板。
  el('manual-toggle').onclick = function () {
    el('manual-panel').classList.toggle('hidden')
  }

  function showError(message) {
    el('error').textContent = message || ''
  }

  function busy(button, label) {
    button.disabled = true
    button.textContent = label
  }
  function unbusy(button, label) {
    button.disabled = false
    button.textContent = label
  }

  function enter(dto) {
    // 配对/选择成功：跳转到本地代理，由代理转发到桌面端。
    if (dto && dto.paired && dto.proxyUrl) {
      window.location.replace(dto.proxyUrl)
    }
  }

  function render(state) {
    var card = el('servers-card')
    var list = el('servers')
    list.innerHTML = ''
    if (!state.servers || !state.servers.length) {
      card.classList.add('hidden')
      return
    }
    card.classList.remove('hidden')
    state.servers.forEach(function (server) {
      var row = document.createElement('div')
      row.className = 'server' + (server.id === state.activeId ? ' active' : '')
      var name = document.createElement('span')
      name.className = 'name'
      name.textContent = server.name
      var ops = document.createElement('span')
      ops.className = 'ops'
      var connect = document.createElement('button')
      connect.className = 'secondary'
      connect.textContent = strings.connect
      connect.onclick = function () {
        busy(connect, strings.connecting)
        invoke('mobile_select_server', { id: server.id }).then(enter).catch(showError).finally(function () {
          unbusy(connect, strings.connect)
        })
      }
      var forget = document.createElement('button')
      forget.className = 'secondary'
      forget.textContent = strings.forget
      forget.onclick = function () {
        if (!window.confirm(strings.forgetConfirm)) return
        invoke('mobile_forget_server', { id: server.id }).then(refresh).catch(showError)
      }
      ops.appendChild(connect)
      ops.appendChild(forget)
      row.appendChild(name)
      row.appendChild(ops)
      list.appendChild(row)
    })
  }

  function refresh() {
    if (!invoke) {
      showError(strings.noTauri)
      return Promise.resolve()
    }
    return invoke('mobile_state')
      .then(function (state) {
        // 本机运行入口始终可用：记录模式记忆后进入壳内本机 Runtime 的对话页。
        el('local-btn').onclick = function () {
          invoke('mobile_enter_local')
            .then(function (updated) {
              if (updated && updated.localUrl) window.location.href = updated.localUrl
            })
            .catch(showError)
        }
        // 冷启动路由由壳内 Rust 侧决定，不经过本页；本页只做服务器/模式管理中枢，
        // 已配对也不再自动跳转，否则「添加服务器」「返回服务器」都无法停留。
        render(state)
      })
      .catch(showError)
  }

  el('scan-btn').onclick = function () {
    if (!invoke) return showError(strings.noTauri)
    showError('')
    var button = el('scan-btn')
    busy(button, strings.pairing)
    var scanner = function (cmd, args) {
      return invoke('plugin:barcode-scanner|' + cmd, args)
    }
    scanner('check_permissions')
      .then(function (perm) {
        if (perm && perm.camera === 'granted') return perm
        return scanner('request_permissions')
      })
      .then(function (perm) {
        if (perm && perm.camera && perm.camera !== 'granted') throw new Error(strings.cameraDenied)
        return scanner('scan', { windowed: false, formats: ['QR_CODE'] })
      })
      .then(function (result) {
        var content = result && result.content
        if (!content) throw new Error(strings.scanFailed)
        return invoke('mobile_pair', { payloadJson: content, deviceName: null })
      })
      .then(enter)
      .catch(function (error) {
        showError(typeof error === 'string' ? error : (error && error.message) || String(error))
      })
      .finally(function () {
        unbusy(button, strings.scan)
      })
  }

  el('manual-btn').onclick = function () {
    if (!invoke) return showError(strings.noTauri)
    showError('')
    var button = el('manual-btn')
    busy(button, strings.pairing)
    invoke('mobile_pair_manual', {
      url: el('input-url').value,
      code: el('input-code').value,
      fingerprint: el('input-fp').value,
      deviceName: null,
    })
      .then(enter)
      .catch(function (error) {
        showError(typeof error === 'string' ? error : (error && error.message) || String(error))
      })
      .finally(function () {
        unbusy(button, strings.manual)
      })
  }

  refresh()
})()

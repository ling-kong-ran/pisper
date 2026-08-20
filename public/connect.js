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
      '把这台设备连接到你的桌面 Pisper。手机与电脑需在同一局域网（或 Tailscale 组网）。',
      'Connect this device to your desktop Pisper. Phone and computer must be on the same network (or a Tailscale network).',
    ),
    servers: t('已配对的桌面端', 'Paired desktops'),
    pairTitle: t('配对新桌面端', 'Pair a new desktop'),
    scan: t('扫码配对', 'Scan QR code to pair'),
    url: t('桌面端地址', 'Desktop address'),
    code: t('配对码', 'Pairing code'),
    fp: t('证书指纹（桌面端设置页可见，可只填前 16 位）', 'Certificate fingerprint (see desktop settings; first 16 hex chars suffice)'),
    manual: t('手动配对', 'Pair manually'),
    manualHint: t(
      '桌面端入口：设置 → 远程访问 → 生成配对二维码。手机上也可手动输入地址、配对码与指纹。',
      'On the desktop: Settings → Remote access → Generate pairing QR code. You can also enter address, code and fingerprint manually.',
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
  el('scan-btn').textContent = strings.scan
  el('label-url').textContent = strings.url
  el('label-code').textContent = strings.code
  el('label-fp').textContent = strings.fp
  el('manual-btn').textContent = strings.manual
  el('manual-hint').textContent = strings.manualHint

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
        // 启动即有激活服务器：直接进入主界面。
        if (state.paired && state.proxyUrl) {
          enter(state)
          return
        }
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

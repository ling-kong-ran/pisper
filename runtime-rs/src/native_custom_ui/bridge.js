// Pisper 自定义 UI 桥：与父页面（应用壳）通过 postMessage 通信。
;(function () {
  if (window.pisper) return
  var nextId = 1
  var pending = new Map()
  var themeListeners = new Set()
  function post(method, params) {
    return new Promise(function (resolve, reject) {
      var id = nextId++
      pending.set(id, { resolve: resolve, reject: reject })
      parent.postMessage({ pisperBridge: 1, id: id, method: method, params: params || {} }, '*')
    })
  }
  window.addEventListener('message', function (event) {
    var data = event.data
    if (event.source !== parent || !data || data.pisperBridge !== 1) return
    if (data.type === 'theme') {
      applyTheme(data.theme || {})
      themeListeners.forEach(function (listener) {
        try {
          listener(data.theme || {})
        } catch (error) {
          console.error(error)
        }
      })
      return
    }
    var slot = pending.get(data.id)
    if (!slot) return
    pending.delete(data.id)
    if (data.ok) {
      // 握手响应携带主题：就绪即应用，保证首帧渲染不闪烁。
      if (data.result && data.result.theme) applyTheme(data.result.theme)
      slot.resolve(data.result)
    } else {
      slot.reject(new Error(String(data.error || 'Pisper bridge call failed')))
    }
  })
  function applyTheme(theme) {
    var root = document.documentElement
    root.dataset.pisperTheme = theme.mode === 'dark' ? 'dark' : 'light'
    root.style.colorScheme = root.dataset.pisperTheme
    var vars = theme.variables || {}
    Object.keys(vars).forEach(function (name) {
      if (/^--[a-z0-9-]+$/i.test(name)) root.style.setProperty(name, String(vars[name]))
    })
  }
  window.pisper = {
    // 握手：返回组件信息、声明的能力与当前主题。
    ready: function () {
      return post('ready')
    },
    getConfig: function () {
      return post('getConfig')
    },
    listSessions: function (params) {
      return post('listSessions', params)
    },
    gameAssets: {
      list: function (params) {
        return post('gameAssets.list', params)
      },
      save: function (params) {
        return post('gameAssets.save', params)
      },
      run: function (params) {
        return post('gameAssets.run', params)
      },
      stop: function (params) {
        return post('gameAssets.stop', params)
      },
      uploadImage: function (params) {
        return post('gameAssets.uploadImage', params)
      },
      image: function (params) {
        return post('gameAssets.image', params)
      },
      process: function (params) {
        return post('gameAssets.process', params)
      },
      engine: function (params) {
        return post('gameAssets.engine', params)
      },
      export: function (params) {
        return post('gameAssets.export', params)
      },
      editFrames: function (params) {
        return post('gameAssets.editFrames', params)
      },
      remove: function (params) {
        return post('gameAssets.remove', params)
      },
    },
    notify: function (message, kind) {
      return post('notify', { message: message, kind: kind })
    },
    onThemeChanged: function (listener) {
      themeListeners.add(listener)
      return function () {
        themeListeners.delete(listener)
      }
    },
  }
})()

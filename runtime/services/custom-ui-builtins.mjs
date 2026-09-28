// 内置示例与用户组件使用同一沙箱和桥接协议；资源随 Runtime 模块打包，离线可用。
/**
 * @typedef {Readonly<{
 *   id: string,
 *   manifest: Readonly<{
 *     name: string, version: string, description: string, entry: string,
 *     permissions: readonly string[],
 *   }>,
 *   assets: Readonly<Record<string, string>>,
 * }>} BuiltinCustomUiComponent
 */

/** @type {readonly BuiltinCustomUiComponent[]} */
export const BUILTIN_CUSTOM_UI_COMPONENTS = Object.freeze([
  Object.freeze({
    id: 'pisper-island',
    manifest: Object.freeze({
      name: 'Pisper Island',
      version: '1.0.0',
      description:
        '灵动岛：时钟、可调整的专注计时与完成提醒 / Clock, adjustable focus timer and reminders',
      entry: 'index.html',
      permissions: Object.freeze(['notify']),
    }),
    assets: Object.freeze({
      'index.html': String.raw`<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>Pisper Island</title>
    <style>
      :root { color-scheme: dark; font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; }
      * { box-sizing: border-box; }
      [hidden] { display: none !important; }
      body { margin: 0; min-width: 0; background: transparent; color: #fff; }
      .stage { display: flex; align-items: center; justify-content: center; min-height: 64px; padding: 6px 8px; }
      .island { width: fit-content; max-width: 100%; min-width: 0; height: 48px; padding: 4px 8px 4px 18px; border: 1px solid #323232; border-radius: 28px; background: #151515; box-shadow: 0 3px 10px #0002; }
      .display { display: flex; align-items: center; gap: 14px; height: 100%; }
      .island[data-editing="true"] { height: 52px; padding: 3px 10px; border-radius: 16px; }
      .clock { font-size: 13px; color: #d4d4d4; font-variant-numeric: tabular-nums; white-space: nowrap; }
      .line { width: 1px; height: 18px; background: #444; flex-shrink: 0; }
      .timer { display: flex; align-items: center; gap: 9px; min-width: 0; }
      .light { width: 6px; height: 6px; border-radius: 50%; background: #777; flex-shrink: 0; }
      .island[data-state="running"] .light { background: #a3e6bd; animation: breathe 2s ease-in-out infinite; }
      .island[data-state="complete"] .light { background: #a3e6bd; }
      .label { font-size: 12px; color: #d4d4d4; white-space: nowrap; }
      .remaining { font-size: 14px; font-weight: 500; font-variant-numeric: tabular-nums; white-space: nowrap; }
      .controls { display: flex; align-items: center; gap: 4px; flex-shrink: 0; }
      button { display: inline-flex; align-items: center; justify-content: center; min-width: 36px; min-height: 36px; border: 0; border-radius: 50%; background: #303030; color: #fff; cursor: pointer; }
      button:hover { background: #424242; }
      button:focus-visible { outline: 2px solid #a3e6bd; outline-offset: 2px; }
      button:disabled { color: #9c9c9c; background: transparent; cursor: default; }
      .edit-duration { gap: 7px; padding: 0 3px; border-radius: 6px; background: transparent; }
      .edit-duration svg { width: 10px; height: 10px; color: #aaa; }
      .settings { width: 318px; max-width: 100%; }
      .settings-top { display: flex; align-items: center; justify-content: space-between; gap: 8px; min-height: 14px; font-size: 10px; color: #c9c9c9; }
      .hint { margin: 0; min-width: 0; }
      .sound { display: inline-flex; align-items: center; gap: 3px; flex-shrink: 0; cursor: pointer; }
      .sound input { margin: 0; accent-color: #a3e6bd; }
      form { display: flex; align-items: center; justify-content: center; gap: 6px; height: 29px; }
      .minutes, select { min-width: 0; height: 25px; border: 1px solid #555; border-radius: 6px; background: #292929; color: #fff; font: inherit; font-size: 12px; }
      .minutes { width: 54px; padding: 2px 4px; }
      .unit { font-size: 11px; color: #d4d4d4; }
      select { width: 86px; }
      form button { min-height: 27px; min-width: 27px; border-radius: 6px; font-size: 11px; padding: 0 7px; }
      .apply { background: #a3e6bd; color: #13241a; }
      .apply:hover { background: #baf3cf; }
      input:focus-visible, select:focus-visible { outline: 2px solid #a3e6bd; outline-offset: 1px; }
      .error { color: #ffd98c; }
      svg { display: block; width: 15px; height: 15px; fill: none; stroke: currentColor; stroke-width: 1.7; stroke-linecap: round; stroke-linejoin: round; }
      .sr-only { position: absolute; width: 1px; height: 1px; padding: 0; overflow: hidden; clip: rect(0, 0, 0, 0); white-space: nowrap; }
      @keyframes breathe { 50% { opacity: .4; } }
      @media (max-width: 360px) { .clock, .line { display: none; } .display { gap: 10px; } .island { padding-left: 14px; } .settings { width: 280px; } }
      @media (max-width: 290px) { .settings { width: 234px; } form { gap: 4px; } select { width: 66px; } .settings-top { font-size: 9px; gap: 4px; } }
      @media (max-width: 230px) { .label { display: none; } .display { gap: 6px; } .island { padding-left: 8px; } .timer { gap: 5px; } .settings { width: 194px; } .minutes { width: 42px; } select { width: 58px; } form { gap: 3px; } form button { padding: 0 4px; } }
      @media (max-width: 190px) { .stage { padding-inline: 0; } .island { padding-inline: 5px; } .display, .timer { gap: 3px; } .edit-duration { gap: 3px; } button { min-width: 30px; min-height: 34px; } .controls { gap: 1px; } .settings { width: 150px; } select, .settings-top .hint { display: none; } .settings-top { justify-content: flex-end; } form button { min-width: 24px; } }
      @media (prefers-reduced-motion: reduce) { .island[data-state="running"] .light { animation: none; } }
    </style>
  </head>
  <body>
    <main class="stage">
      <section id="island" class="island" data-state="idle" aria-label="Focus timer">
        <div id="timer-display" class="display">
        <time id="clock" class="clock"></time>
        <span class="line" aria-hidden="true"></span>
        <div class="timer">
          <span class="light" aria-hidden="true"></span>
          <button id="edit-duration" class="edit-duration" type="button" aria-controls="duration-settings" aria-expanded="false" aria-describedby="timer-purpose">
            <span id="label" class="label">Focus</span>
            <span id="remaining" class="remaining" role="timer" aria-live="off">25:00</span>
            <svg viewBox="0 0 12 12" aria-hidden="true"><path d="m3 4.5 3 3 3-3" /></svg>
          </button>
        </div>
        <div class="controls">
          <button id="toggle" type="button" aria-label="Start focus timer" title="Start focus timer">
            <svg id="play" viewBox="0 0 20 20" aria-hidden="true"><path d="m7 4 9 6-9 6Z" /></svg>
            <svg id="pause" viewBox="0 0 20 20" aria-hidden="true" style="display:none"><path d="M7 4v12M13 4v12" /></svg>
          </button>
          <button id="reset" type="button" aria-label="Reset timer" title="Reset timer" disabled>
            <svg viewBox="0 0 20 20" aria-hidden="true"><path d="M4 7a6 6 0 1 1 0 6M4 3v4h4" /></svg>
          </button>
        </div>
        </div>
        <div id="duration-settings" class="settings" hidden>
          <div class="settings-top">
            <p id="settings-hint" class="hint"></p>
            <p id="settings-error" class="hint error" role="alert" hidden></p>
            <label class="sound"><input id="sound-enabled" type="checkbox" checked /><span id="sound-label">Sound</span></label>
          </div>
          <form id="duration-form" novalidate>
            <input id="duration-input" class="minutes" type="number" min="1" max="180" step="1" inputmode="numeric" aria-describedby="settings-hint settings-error" />
            <span id="minutes-unit" class="unit">min</span>
            <select id="duration-presets">
              <option id="preset-placeholder" value="">Presets</option>
              <option value="15">15</option><option value="25">25</option><option value="45">45</option>
            </select>
            <button id="apply-duration" class="apply" type="button">Set</button>
            <button id="cancel-duration" type="button">Cancel</button>
          </form>
        </div>
        <span id="timer-purpose" class="sr-only"></span>
        <span id="announcement" class="sr-only" role="status" aria-live="polite"></span>
      </section>
    </main>
    <script src="/api/custom-ui/bridge.js"></script>
    <script>
      (function () {
        var duration = 25 * 60 * 1000
        var remaining = duration
        var deadline = 0
        var state = 'idle'
        var editing = false
        var invalidDuration = false
        var soundEnabled = true
        var canRemind = false
        var audioContext = null
        var locale = navigator.language || 'en'
        var messages = {
          en: { focus: 'Focus', done: 'Take a break', start: 'Start focus timer', pause: 'Pause focus timer', resume: 'Resume focus timer', restart: 'Start another focus session', reset: 'Reset timer', title: 'Focus timer', running: 'Focus timer started', paused: 'Focus timer paused', complete: 'Focus session complete', resetDone: 'Timer reset', edit: 'Change focus duration', purpose: 'A break reminder while Pisper is open, including when switching pages. Click the time to change it. The timer does not stop the AI conversation.', hint: 'Break reminder · Chat continues', changing: 'Setting a time ends this timer', minutes: 'Duration in minutes', unit: 'min', presets: 'Presets', set: 'Set', cancel: 'Cancel', sound: 'Sound', soundHint: 'Play a short chime when the timer ends', invalid: 'Use a whole number from 1 to 180', saved: 'Duration updated. Start when ready.', reminder: '{minutes} minutes of focus complete. Take a break.' },
          zh: { focus: '专注', done: '休息一下', start: '开始专注', pause: '暂停专注', resume: '继续专注', restart: '再次开始专注', reset: '重置计时', title: '专注计时', running: '专注计时已开始', paused: '专注计时已暂停', complete: '本次专注已完成', resetDone: '计时已重置', edit: '设置专注时长', purpose: '在 Pisper 打开期间提醒自己到点休息，切换页面也会继续计时。点击时间可修改时长，不会停止 AI 会话。', hint: '到点提醒休息，不影响会话', changing: '设定新时长将结束本轮计时', minutes: '专注分钟数', unit: '分钟', presets: '常用时长', set: '设定', cancel: '取消', sound: '声音', soundHint: '结束时播放一声短提示音', invalid: '请输入 1–180 的整数分钟', saved: '时长已更新，点击开始计时。', reminder: '{minutes} 分钟专注已完成，休息一下吧。' }
        }
        var island = document.getElementById('island')
        var clock = document.getElementById('clock')
        var label = document.getElementById('label')
        var counter = document.getElementById('remaining')
        var toggle = document.getElementById('toggle')
        var reset = document.getElementById('reset')
        var play = document.getElementById('play')
        var pause = document.getElementById('pause')
        var announcement = document.getElementById('announcement')
        var display = document.getElementById('timer-display')
        var editDuration = document.getElementById('edit-duration')
        var settings = document.getElementById('duration-settings')
        var form = document.getElementById('duration-form')
        var input = document.getElementById('duration-input')
        var presets = document.getElementById('duration-presets')
        var applyDurationButton = document.getElementById('apply-duration')
        var cancelDuration = document.getElementById('cancel-duration')
        var hint = document.getElementById('settings-hint')
        var error = document.getElementById('settings-error')
        var sound = document.getElementById('sound-enabled')
        var soundLabel = document.getElementById('sound-label')
        function words() { return messages[/^zh/i.test(locale) ? 'zh' : 'en'] }
        function unlockAudio() {
          if (!soundEnabled || !canRemind) return
          try {
            var Audio = window.AudioContext || window.webkitAudioContext
            if (!Audio) return
            if (!audioContext || audioContext.state === 'closed') audioContext = new Audio()
            if (audioContext.state === 'suspended') audioContext.resume().catch(function () {})
          } catch (_) { audioContext = null }
        }
        function chime() {
          if (!soundEnabled || !canRemind || !audioContext || audioContext.state !== 'running') return
          try {
            var start = audioContext.currentTime
            ;[660, 880].forEach(function (frequency, index) {
              var oscillator = audioContext.createOscillator()
              var gain = audioContext.createGain()
              var time = start + index * .22
              oscillator.type = 'sine'
              oscillator.frequency.setValueAtTime(frequency, time)
              gain.gain.setValueAtTime(0, time)
              gain.gain.linearRampToValueAtTime(.12, time + .02)
              gain.gain.linearRampToValueAtTime(0, time + .2)
              oscillator.connect(gain)
              gain.connect(audioContext.destination)
              oscillator.onended = function () { oscillator.disconnect(); gain.disconnect() }
              oscillator.start(time)
              oscillator.stop(time + .21)
            })
          } catch (_) { /* 声音不可用时仍保留完成态与应用通知。 */ }
        }
        function remind() {
          announcement.textContent = words().complete
          if (canRemind && window.pisper) {
            var message = words().reminder.replace('{minutes}', String(duration / 60000))
            window.pisper.notify(message).catch(function () {})
          }
          chime()
        }
        function refresh() {
          if (state === 'running') {
            remaining = Math.max(0, deadline - Date.now())
            if (remaining === 0) {
              state = 'complete'
              remind()
            }
          }
          var seconds = Math.ceil(remaining / 1000)
          counter.textContent = String(Math.floor(seconds / 60)).padStart(2, '0') + ':' + String(seconds % 60).padStart(2, '0')
          var now = new Date()
          clock.textContent = now.toLocaleTimeString(locale, { hour: '2-digit', minute: '2-digit', hour12: false })
          clock.dateTime = now.toISOString()
          var text = words()
          label.textContent = state === 'complete' ? text.done : text.focus
          island.dataset.state = state
          island.setAttribute('aria-label', text.title)
          var action = state === 'running' ? text.pause : state === 'paused' ? text.resume : state === 'complete' ? text.restart : text.start
          toggle.setAttribute('aria-label', action)
          toggle.title = action
          reset.setAttribute('aria-label', text.reset)
          reset.title = text.reset
          reset.disabled = state === 'idle'
          editDuration.setAttribute('aria-label', text.edit)
          editDuration.title = text.purpose
          document.getElementById('timer-purpose').textContent = text.purpose
          hint.textContent = state === 'running' || state === 'paused' ? text.changing : text.hint
          hint.hidden = invalidDuration
          error.hidden = !invalidDuration
          error.textContent = invalidDuration ? text.invalid : ''
          input.setAttribute('aria-label', text.minutes)
          input.setAttribute('aria-invalid', String(invalidDuration))
          document.getElementById('minutes-unit').textContent = text.unit
          presets.setAttribute('aria-label', text.presets)
          document.getElementById('preset-placeholder').textContent = text.presets
          applyDurationButton.textContent = text.set
          cancelDuration.textContent = text.cancel
          soundLabel.textContent = text.sound
          sound.setAttribute('aria-label', text.soundHint)
          sound.title = text.soundHint
          play.style.display = state === 'running' ? 'none' : 'block'
          pause.style.display = state === 'running' ? 'block' : 'none'
        }
        toggle.addEventListener('click', function () {
          refresh()
          if (state === 'running') {
            state = 'paused'
            announcement.textContent = words().paused
          } else {
            if (state === 'complete') remaining = duration
            deadline = Date.now() + remaining
            state = 'running'
            // 必须在点击手势内解锁声音，计时本身不等待浏览器音频许可。
            unlockAudio()
            announcement.textContent = words().running
          }
          refresh()
        })
        function closeSettings() {
          editing = false
          invalidDuration = false
          display.hidden = false
          settings.hidden = true
          island.dataset.editing = 'false'
          editDuration.setAttribute('aria-expanded', 'false')
          refresh()
          editDuration.focus()
        }
        editDuration.addEventListener('click', function () {
          editing = true
          invalidDuration = false
          input.value = String(duration / 60000)
          presets.value = ''
          sound.checked = soundEnabled
          display.hidden = true
          settings.hidden = false
          island.dataset.editing = 'true'
          editDuration.setAttribute('aria-expanded', 'true')
          refresh()
          input.focus()
          input.select()
        })
        presets.addEventListener('change', function () {
          if (presets.value) input.value = presets.value
          invalidDuration = false
          refresh()
        })
        input.addEventListener('input', function () {
          invalidDuration = false
          presets.value = ''
          refresh()
        })
        function applyDuration(event) {
          event.preventDefault()
          var minutes = Number(input.value)
          if (!input.value.trim() || !Number.isInteger(minutes) || minutes < 1 || minutes > 180) {
            invalidDuration = true
            refresh()
            input.focus()
            return
          }
          duration = minutes * 60000
          remaining = duration
          deadline = 0
          state = 'idle'
          soundEnabled = sound.checked
          closeSettings()
          announcement.textContent = words().saved
        }
        // 沙箱不允许表单导航，浏览器可能在 submit 事件前阻止提交；直接处理按钮和回车。
        applyDurationButton.addEventListener('click', applyDuration)
        form.addEventListener('submit', applyDuration)
        cancelDuration.addEventListener('click', closeSettings)
        form.addEventListener('keydown', function (event) {
          if (event.key === 'Escape' && editing) {
            event.preventDefault()
            closeSettings()
          } else if (event.key === 'Enter' && editing && (event.target === input || event.target === applyDurationButton)) {
            applyDuration(event)
          }
        })
        reset.addEventListener('click', function () {
          state = 'idle'
          remaining = duration
          deadline = 0
          announcement.textContent = words().resetDone
          refresh()
        })
        function setLocale(value) {
          if (typeof value !== 'string' || !value.trim()) return
          try { new Date().toLocaleTimeString(value) } catch (_) { return }
          locale = value
          document.documentElement.lang = value
          refresh()
        }
        refresh()
        var timer = setInterval(refresh, 1000)
        window.addEventListener('pagehide', function () {
          clearInterval(timer)
          timer = null
          if (audioContext) {
            audioContext.close().catch(function () {})
            audioContext = null
          }
        })
        window.addEventListener('pageshow', function () {
          if (timer === null) { refresh(); timer = setInterval(refresh, 1000) }
        })
        document.addEventListener('visibilitychange', function () {
          if (!document.hidden) refresh()
        })
        if (window.pisper) {
          window.pisper.ready().then(function (ready) {
            canRemind = Boolean(ready && ready.component && Array.isArray(ready.component.permissions) && ready.component.permissions.indexOf('notify') !== -1)
            setLocale(ready && ready.locale)
          }).catch(function () {})
          window.pisper.onThemeChanged(function (theme) { setLocale(theme && theme.locale) })
        }
      })()
    </script>
  </body>
</html>`,
    }),
  }),
])

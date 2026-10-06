// Self-contained browser function: Root serializes it into an explicitly isolated
// packaged WebView at document initialization. No controlled IPC is installed.
export async function checkNativeTerminalDom({ workspace, sentinel, sessionId }) {
  const evidence = {
    layer: 'actual-packaged-webview-react-xterm-native-pty',
    controlledIpc: false,
    input: 'programmatic untrusted DOM InputEvent and KeyboardEvent',
    clicks: 'programmatic untrusted DOM click events',
    physicalKeyboardVerified: false,
    imeVerified: false,
    dsrReplyPath: 'unmodified production xterm onData path',
    manualDsrReplies: false,
    dsrRepliesInstrumented: false,
    steps: [],
    api: [],
    pageErrors: [],
    activeSessionEvents: [],
  }
  const ensure = (condition, message) => {
    if (!condition) throw new Error(message)
  }
  ensure(
    window === window.top && location.hostname === '127.0.0.1',
    'Use an owned top-level loopback WebView',
  )
  ensure(
    typeof workspace === 'string' && /^(?:[A-Za-z]:[\\/]|\\\\[^\\]+\\[^\\]+|\/)/.test(workspace),
    'The synthetic workspace must be absolute',
  )
  ensure(
    typeof sentinel === 'string' && /^[A-Za-z][A-Za-z0-9_-]{7,64}$/.test(sentinel),
    'Use an ASCII-only synthetic sentinel',
  )
  ensure(!window.__terminalContract, 'Controlled IPC cannot prove native terminal DOM behavior')
  // This runs synchronously before the first await, so production startup marks
  // are recorded. A caller that invokes it after startup must not fake marks.
  localStorage.setItem('pisper:startup-diagnostics', '1')
  localStorage.setItem('pisper-model-onboarding-v1-dismissed', '1')
  const started = performance.now()
  const deadline = Date.now() + 75_000
  const normalizePath = (value) =>
    String(value)
      .replace(/^\\\\\?\\/, '')
      .replaceAll('/', '\\')
      .replace(/\\+$/, '')
      .toLowerCase()
  const visible = (element) =>
    Boolean(
      element?.isConnected &&
      element.getClientRects().length &&
      getComputedStyle(element).visibility !== 'hidden' &&
      getComputedStyle(element).display !== 'none',
    )
  const find = (selector, area = document) => [...area.querySelectorAll(selector)].find(visible)
  const panel = () => document.querySelector('.terminal-panel')
  const rows = () => panel()?.querySelector('.terminal-host.active .xterm-rows')?.textContent || ''
  const compactRows = () => rows().replace(/\s+/g, '')
  const status = () => panel()?.querySelector('.terminal-tab.active i[data-status]')?.dataset.status
  const step = (name) =>
    evidence.steps.push({ name, elapsedMs: Math.round(performance.now() - started) })
  let cleaning = false
  const wait = async (name, condition, timeout = 15_000) => {
    const end = cleaning
      ? Date.now() + Math.min(timeout, 3000)
      : Math.min(deadline, Date.now() + timeout)
    while (Date.now() < end) {
      const value = condition()
      if (value) return value
      if (!cleaning && status() === 'error')
        throw new Error(`Native terminal entered error while ${name}`)
      await new Promise((resolve) => setTimeout(resolve, 50))
    }
    throw new Error(`Native terminal DOM deadline: ${name}`)
  }
  const api = async (route, method = 'GET', body) => {
    ensure(route.startsWith('/api/'), 'Use same-origin synthetic API paths')
    const response = await fetch(route, {
      method,
      credentials: 'same-origin',
      redirect: 'error',
      headers: body === undefined ? undefined : { 'Content-Type': 'application/json' },
      body: body === undefined ? undefined : JSON.stringify(body),
      signal: AbortSignal.timeout(10_000),
    })
    evidence.api.push({ route, method, status: response.status })
    const value = response.status === 204 ? undefined : await response.json()
    ensure(
      response.ok,
      `Synthetic API ${method} ${route} returned ${response.status}${value?.code ? ` (${value.code})` : ''}`,
    )
    return value
  }
  const dismissOnboarding = async () => {
    const dialog = find('[role="dialog"][aria-describedby="model-onboarding-description"]')
    if (!dialog) return
    const close = [...dialog.querySelectorAll('button[aria-label]')].find((button) =>
      button.querySelector('svg.lucide-x'),
    )
    ensure(close, 'The actual model onboarding dialog must offer its dismissal button')
    close.click()
    await wait('actual onboarding dialog dismissal', () => !visible(dialog), 3000)
    evidence.onboardingDismissedThroughDialog = true
  }
  const click = async (name, selector, area = document) => {
    await dismissOnboarding()
    const element = await wait(name, () => find(selector, area))
    ensure(
      !element.disabled && !element.closest('[inert]'),
      `${name} must be enabled in the real UI`,
    )
    if (element instanceof HTMLElement) element.click()
    else
      element.dispatchEvent(
        new MouseEvent('click', { bubbles: true, cancelable: true, view: window }),
      )
    step(name)
    return element
  }
  const toggleButton = () =>
    [...document.querySelectorAll('button[aria-label]')].find(
      (button) =>
        visible(button) &&
        !button.closest('.terminal-panel') &&
        button.querySelector('svg.lucide-square-terminal, svg.lucide-terminal-square'),
    )
  const selectSession = async (id) => {
    window.dispatchEvent(
      new CustomEvent('pisper:session-selected', {
        detail: { sessionId: id, disposition: 'open' },
      }),
    )
    await wait(
      `production session selection ${id}`,
      () => localStorage.getItem('pisper-active-session') === id,
    )
    step('production session-selected event consumed')
  }
  const command = async (text) => {
    ensure(/^[\x20-\x7e]+$/.test(text), 'Only ASCII PowerShell expressions are entered')
    const textarea = await wait('real xterm input textarea', () =>
      panel()?.querySelector('.terminal-host.active .xterm-helper-textarea'),
    )
    textarea.focus()
    ensure(document.activeElement === textarea, 'The production xterm textarea must receive focus')
    const input = new InputEvent('input', {
      data: text,
      inputType: 'insertText',
      bubbles: true,
      cancelable: true,
      composed: false,
    })
    ensure(input.isTrusted === false, 'Record the actual programmatic input boundary')
    textarea.value = text
    textarea.dispatchEvent(input)
    const enter = new KeyboardEvent('keydown', {
      key: 'Enter',
      code: 'Enter',
      keyCode: 13,
      which: 13,
      bubbles: true,
      cancelable: true,
    })
    ensure(enter.isTrusted === false, 'Record the actual programmatic keyboard boundary')
    textarea.dispatchEvent(enter)
    textarea.dispatchEvent(
      new KeyboardEvent('keyup', {
        key: 'Enter',
        code: 'Enter',
        keyCode: 13,
        which: 13,
        bubbles: true,
      }),
    )
    step('untrusted DOM input passed through production xterm')
  }
  const onError = (event) =>
    evidence.pageErrors.push(String(event.message || 'Uncaught WebView error'))
  const onRejection = (event) =>
    evidence.pageErrors.push(String(event.reason?.message || 'Unhandled WebView rejection'))
  const onSession = (event) => evidence.activeSessionEvents.push(event.detail?.id || '')
  window.addEventListener('error', onError)
  window.addEventListener('unhandledrejection', onRejection)
  window.addEventListener('pisper:active-session-changed', onSession)
  let failure
  const cleanupErrors = []
  let secondSessionId
  let selectedId
  let bridge
  try {
    await wait(
      'actual production desktop bridge',
      () => window.pisperDesktop?.terminalProfiles && window.__TAURI_INTERNALS__?.invoke,
    )
    bridge = window.pisperDesktop
    evidence.appInfo = await bridge.getAppInfo()
    ensure(
      evidence.appInfo.desktop === true && evidence.appInfo.packaged === true,
      'Use the actual packaged desktop host',
    )
    const health = await api('/api/health')
    evidence.engine = health.engine
    ensure(health.engine === 'pi-rs', 'The owned native WebView must use the Rust backend')
    const capabilities = await api('/api/runtime/capabilities')
    evidence.backendTerminalCapability = capabilities.features?.terminal
    ensure(
      evidence.backendTerminalCapability === true,
      'The real backend terminal capability must be enabled',
    )
    const catalog = await api('/api/sessions')
    const sessions = Array.isArray(catalog) ? catalog : catalog.sessions
    ensure(Array.isArray(sessions), 'The owned bootstrap session catalog must exist')
    const selected = sessionId
      ? sessions.find((session) => session.id === sessionId)
      : sessions.find((session) => normalizePath(session.cwd) === normalizePath(workspace))
    ensure(
      selected?.id && normalizePath(selected.cwd) === normalizePath(workspace),
      'Select the owned bootstrap session in the synthetic workspace',
    )
    selectedId = selected.id
    evidence.sessionId = selectedId
    evidence.cwd = selected.cwd
    const updates = {
      'pisper-active-session': selectedId,
      'pisper-terminal-panel': JSON.stringify({ open: false, height: 300 }),
      'pisper-model-onboarding-v1-dismissed': '1',
    }
    await api('/api/local/browser-preferences', 'PUT', { updates })
    for (const [key, value] of Object.entries(updates)) localStorage.setItem(key, value)
    const stored = await api('/api/local/browser-preferences')
    ensure(
      Object.entries(updates).every(([key, value]) => stored.values?.[key] === value),
      'Synthetic preferences must persist through the real API',
    )
    if (location.hash !== '#/chat') location.hash = '#/chat'
    evidence.startupPhases = [
      'react-app-mounted',
      'client-info-loaded',
      'capabilities-loaded',
      'config-loaded',
      'sessions-loaded',
      'chat-shell-ready',
    ]
    await wait(
      'production startup markers',
      () =>
        evidence.startupPhases.every(
          (phase) => performance.getEntriesByName(`pisper-${phase}`).length,
        ),
      20_000,
    )
    await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)))
    await dismissOnboarding()
    await selectSession(selectedId)
    const toggle = await wait('real React terminal entry', toggleButton)
    ensure(!toggle.disabled, 'The real React terminal entry must be enabled')
    if (visible(panel()))
      await click('close any initial empty terminal panel', '.terminal-title', panel())
    toggle.click()
    await wait('actual TerminalPanel visibility', () => visible(panel()))
    ensure(
      panel().querySelectorAll('.terminal-tab').length === 0,
      'Use a fresh owned GUI with no pre-existing terminal tabs',
    )
    const profiles = await bridge.terminalProfiles()
    const profile =
      profiles.find((value) => value.id === 'powershell') ||
      profiles.find((value) => value.id === 'pwsh')
    ensure(profile, 'This native DOM proof requires a discovered PowerShell profile')
    evidence.profileId = profile.id
    if (profiles[0]?.id === profile.id) {
      await click(
        'React empty-panel creates native PowerShell',
        '.terminal-empty:not([disabled])',
        panel(),
      )
    } else {
      const plus = await wait('React new-terminal menu', () =>
        [...panel().querySelectorAll('button[aria-label]')].find(
          (button) =>
            visible(button) && !button.disabled && button.querySelector('svg.lucide-plus'),
        ),
      )
      plus.click()
      const choice = await wait('native PowerShell profile menu choice', () =>
        [...panel().querySelectorAll('.terminal-profile-menu button')].find(
          (button) => visible(button) && button.textContent.trim() === profile.label,
        ),
      )
      choice.click()
      step('React profile menu creates native PowerShell')
    }
    await wait('native terminal running status', () => status() === 'running')
    const originalXterm = await wait('actual xterm renderer', () =>
      find('.terminal-host.active .xterm', panel()),
    )
    await wait('PowerShell prompt rendered by xterm without manual DSR replies', () =>
      rows().includes('PS '),
    )
    ensure(
      normalizePath(panel().querySelector('.terminal-tab.active')?.title) ===
        normalizePath(workspace),
      'TerminalPanel must display the actual workspace cwd',
    )
    const liveTag = `${sentinel}_LIVE_`
    const cwdTag = `${sentinel}_CWD_B64:`
    await command(
      `[Console]::OutputEncoding=[System.Text.UTF8Encoding]::new($false); Write-Output ('${liveTag}'+$PID); Write-Output ('${cwdTag}'+[Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes((Get-Location).Path))+'|CWD_END|')`,
    )
    const live = await wait('actual PowerShell PID output', () =>
      new RegExp(`${liveTag}(\\d+)`).exec(compactRows()),
    )
    evidence.nativeShellPid = Number(live[1])
    ensure(
      Number.isInteger(evidence.nativeShellPid) &&
        evidence.nativeShellPid > 0 &&
        evidence.nativeShellPid !== 30488,
      'The actual shell must be an owned native process',
    )
    const cwdLine = await wait('actual cwd encoded by PowerShell', () =>
      new RegExp(`${cwdTag}([A-Za-z0-9+/]+={0,2})\\|CWD_END\\|`).exec(compactRows()),
    )
    evidence.actualCwd = new TextDecoder().decode(
      Uint8Array.from(atob(cwdLine[1]), (character) => character.charCodeAt(0)),
    )
    ensure(
      normalizePath(evidence.actualCwd) === normalizePath(workspace),
      'Actual native Get-Location must equal the synthetic workspace',
    )
    evidence.runningObserved = true
    await click('hide real terminal panel', '.terminal-title', panel())
    await wait('terminal pane hidden', () => !visible(panel()))
    toggleButton().click()
    await wait(
      'terminal pane reopened',
      () => visible(panel()) && find('.terminal-host.active .xterm', panel()),
    )
    ensure(
      panel().querySelector('.terminal-host.active .xterm') === originalXterm &&
        status() === 'running',
      'Hide/reopen must retain the same running native xterm instance',
    )
    ensure(
      compactRows().includes(`${liveTag}${evidence.nativeShellPid}`),
      'Hide/reopen must retain rendered native output',
    )
    evidence.hideReopenSameXterm = true
    const second = await api('/api/sessions', 'POST', {
      name: `Native terminal DOM ${sentinel}`,
      cwd: workspace,
    })
    ensure(
      typeof second.id === 'string' && second.id !== selectedId,
      'Create a separate owned synthetic UI session',
    )
    secondSessionId = second.id
    evidence.switchedSessionId = secondSessionId
    window.dispatchEvent(new CustomEvent('pisper:sessions-updated'))
    await selectSession(secondSessionId)
    await wait(
      'second session has an empty scoped terminal pane',
      () =>
        find('.terminal-empty:not([disabled])', panel()) &&
        panel().querySelectorAll('.terminal-tab').length === 0,
    )
    await selectSession(selectedId)
    await wait(
      'original session reattaches its running terminal',
      () =>
        panel()?.querySelector('.terminal-host.active .xterm') === originalXterm &&
        status() === 'running',
    )
    const resumedTag = `${sentinel}_RESUMED_`
    await command(`Write-Output ('${resumedTag}'+$PID)`)
    await wait('same native PID after session switching', () =>
      compactRows().includes(`${resumedTag}${evidence.nativeShellPid}`),
    )
    evidence.sessionSwitchRetainedSamePid = true
    const utf8Tag = `${sentinel}_UTF8_`
    await command(`Write-Output ('${utf8Tag}'+[char]0x4e2d); exit 7`)
    await wait(
      'native Exit status through the actual React panel',
      () => status() === 'exited',
      20_000,
    )
    await wait(
      'native UTF-8 and exit 7 rendered by xterm',
      () =>
        compactRows().includes(`${utf8Tag}中`) &&
        /Processexitedwithcode7|进程已退出，代码7/.test(compactRows()),
    )
    evidence.exitCode = 7
    evidence.utf8OutputVerified = true
    evidence.output = rows()
    evidence.exitedObserved = true
    await click('React close terminal tab', '.terminal-tab.active svg[aria-label]', panel())
    await wait(
      'React terminal tab removed after actual close',
      () => panel().querySelectorAll('.terminal-tab').length === 0,
    )
    evidence.uiCloseVerified = true
    ensure(
      evidence.pageErrors.length === 0,
      'The production WebView must not report uncaught page errors',
    )
  } catch (error) {
    failure = error
    evidence.diagnostic = {
      hash: location.hash,
      terminalStatus: status(),
      terminalRows: rows(),
      visibleDialog: find('[role="dialog"]')?.textContent?.slice(0, 1200),
      startupMarks: performance
        .getEntriesByType('mark')
        .filter((entry) => entry.name.startsWith('pisper-'))
        .map((entry) => entry.name),
    }
  } finally {
    cleaning = true
    for (const cleanup of [
      async () => {
        if (bridge) {
          await bridge.terminalCloseAll()
          evidence.terminalsClosed = true
        }
      },
      async () => {
        if (selectedId && secondSessionId) await selectSession(selectedId)
      },
      async () => {
        if (secondSessionId)
          await api(`/api/sessions/${encodeURIComponent(secondSessionId)}`, 'DELETE')
      },
    ]) {
      try {
        await cleanup()
      } catch (error) {
        cleanupErrors.push(error)
      }
    }
    window.removeEventListener('error', onError)
    window.removeEventListener('unhandledrejection', onRejection)
    window.removeEventListener('pisper:active-session-changed', onSession)
  }
  evidence.elapsedMs = Math.round(performance.now() - started)
  if (!failure && evidence.pageErrors.length) {
    failure = new Error('The production WebView reported uncaught errors during cleanup')
  }
  if (failure) {
    failure.evidence = evidence
    failure.cleanupErrors = cleanupErrors.map((error) => error.message)
    throw failure
  }
  if (cleanupErrors.length) {
    const error = new AggregateError(cleanupErrors, 'Native terminal DOM cleanup failed')
    error.evidence = evidence
    error.cleanupErrors = cleanupErrors.map((cause) => cause.message)
    throw error
  }
  return evidence
}

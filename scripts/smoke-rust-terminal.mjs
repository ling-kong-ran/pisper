import assert from 'node:assert/strict'
import { mkdir, readFile, rmdir } from 'node:fs/promises'
import { randomUUID } from 'node:crypto'
import { isAbsolute, join } from 'node:path'

const nativeTests = [
  'native_pty_output_utf8_cwd_resize_input_and_exit_are_real',
  'native_pty_close_all_reaps_then_reopens_and_shutdown_blocks_late_spawn',
  'native_pty_disconnected_event_channel_kills_and_reaps_shell',
  'native_pty_invalid_spawn_and_duplicate_admission_leave_no_extra_child',
]

// This path requires the packaged host's own WebView and production IPC. It
// never installs the controlled transport used by the independent UI contract.
export async function checkNativeDesktopTerminalIpcParity({ check, page, workspace }) {
  assert.ok(isAbsolute(workspace))
  await check('Actual packaged WebView2 terminal IPC executes and reaps a native PTY', async () => {
    const terminalId = `native-webview-${randomUUID()}`
    const sentinel = `NATIVE_WEBVIEW_${randomUUID().replaceAll('-', '')}`
    try {
      const startup = await page.evaluate(async () => {
        if (window.__terminalContract)
          throw new Error('A controlled transport cannot prove native IPC')
        const bridge = window.pisperDesktop
        return { app: await bridge.getAppInfo(), profiles: await bridge.terminalProfiles() }
      })
      assert.equal(startup.app.desktop, true)
      assert.equal(startup.app.packaged, true)
      const profile =
        startup.profiles.find((value) => value.id === 'powershell') ||
        startup.profiles.find((value) => value.id === 'pwsh')
      assert.ok(profile, 'This Windows native IPC proof requires an actual PowerShell profile')
      const created = await page.evaluate(
        ({ terminalId, profileId, cwd }) => {
          const probe = { terminalId, text: '', events: [], exit: null, cursorQueriesAnswered: 0 }
          window.__nativePtySmoke = probe
          const decoder = new TextDecoder()
          let inspectedOutput = 0
          return window.pisperDesktop.terminalCreate(
            { terminalId, profileId, cwd, cols: 80, rows: 24 },
            (event) => {
              probe.events.push(event.type)
              if (event.type === 'output') {
                probe.text += decoder.decode(Uint8Array.from(event.data), { stream: true })
                let query
                while ((query = probe.text.indexOf('\u001b[6n', inspectedOutput)) >= 0) {
                  inspectedOutput = query + 4
                  window.pisperDesktop
                    .terminalWrite(terminalId, new TextEncoder().encode('\u001b[1;1R'))
                    .then(() => {
                      probe.cursorQueriesAnswered += 1
                    })
                    .catch((error) => {
                      probe.error = error.message
                    })
                }
              }
              if (event.type === 'exit') probe.exit = event.code
              if (event.type === 'error') probe.error = event.message
            },
          )
        },
        { terminalId, profileId: profile.id, cwd: workspace },
      )
      assert.equal(created.terminalId, terminalId)
      assert.equal(created.cwd, workspace)
      await page.waitForFunction(() => window.__nativePtySmoke?.text.includes('PS '), undefined, {
        timeout: 10_000,
      })
      const command = `[Console]::OutputEncoding=[System.Text.UTF8Encoding]::new($false); Write-Output ('${sentinel}_' + [char]0x4e2d); Write-Output ((Get-Location).Path); exit 7\r`
      await page.evaluate(
        async ({ terminalId, command }) => {
          await window.pisperDesktop.terminalResize(terminalId, 101, 31)
          await window.pisperDesktop.terminalWrite(terminalId, new TextEncoder().encode(command))
        },
        { terminalId, command },
      )
      await page.waitForFunction(() => window.__nativePtySmoke?.exit !== null, undefined, {
        timeout: 20_000,
      })
      const result = await page.evaluate(() => window.__nativePtySmoke)
      assert.equal(result.error, undefined)
      assert.equal(result.exit, 7)
      assert.ok(
        result.text.includes(`${sentinel}_中`),
        'Actual PTY output must retain split UTF-8 bytes',
      )
      assert.ok(
        result.text.includes(workspace),
        'Actual native shell must start in the owned workspace',
      )
      assert.ok(result.events.includes('output') && result.events.includes('exit'))
      return {
        layer: 'actual-packaged-webview-native-pty',
        controlledIpc: false,
        nativeBridge: true,
        profile: profile.id,
        exitCode: result.exit,
        utf8OutputVerified: true,
        cwdVerified: true,
        resizeInvoked: true,
      }
    } finally {
      if (!page.isClosed()) {
        await page.evaluate(async () => {
          await window.pisperDesktop.terminalCloseAll()
          delete window.__nativePtySmoke
        })
      }
    }
  })
}

// Evidence comes from Root's actual isolated src-tauri Cargo test invocation.
// This module does not launch Cargo, touch the installed GUI, or provide a shell.
export async function checkNativeTerminalEvidence({ check, nativeEvidence }) {
  let verified = false
  await check('Native desktop PTY lifecycle evidence (actual Rust/OS tests)', () => {
    assert.equal(nativeEvidence?.exitCode, 0, 'Native PTY tests must have exited successfully')
    assert.ok(
      ['win32', 'linux', 'darwin'].includes(nativeEvidence.platform),
      'Record the actual native test platform',
    )
    assert.match(String(nativeEvidence.command), /src-tauri.*Cargo\.toml/)
    assert.match(String(nativeEvidence.command), /desktop_terminal::tests/)
    const output = String(nativeEvidence.output || '')
    const required = [...nativeTests]
    if (nativeEvidence.platform === 'win32') {
      required.push('native_pty_windows_job_close_reaps_spawned_descendant')
    }
    for (const name of required) {
      assert.match(output, new RegExp(`${name} [.]{3} ok(?:\r?\n|$)`), name)
    }
    assert.match(output, /test result: ok\./)
    verified = true
    return {
      layer: 'native-pty',
      platform: nativeEvidence.platform,
      tests: required,
      installedWebviewTested: false,
    }
  })
  return verified
}

// A separate fresh synthetic browser context is required. The transport below
// only checks production desktop-bridge.js + TerminalPanel wiring. It never
// claims to execute a native shell: that evidence is mandatory and independent.
export async function checkTerminalBridgeUiParity({
  check,
  page,
  base,
  nativeEvidence,
  isolatedContext = false,
  json,
  workspace,
}) {
  const verified = await checkNativeTerminalEvidence({ check, nativeEvidence })
  if (!verified) return { layer: 'frontend-bridge-contract', skipped: 'Native evidence failed' }
  assert.equal(isolatedContext, true, 'Use a fresh owned synthetic browser context')
  assert.equal(typeof json, 'function', 'Pass the authenticated synthetic backend JSON helper')
  assert.equal(typeof workspace, 'string', 'Pass the owned synthetic workspace')
  assert.ok(isAbsolute(workspace), 'The owned synthetic workspace must be absolute')
  const productionBridge = await readFile(
    new URL('../src-tauri/src/desktop_shell/desktop-bridge.js', import.meta.url),
    'utf8',
  )
  await page.addInitScript({
    content: `;(${installControlledTransport.toString()})();\n${productionBridge}`,
  })
  await check(
    'Production terminal UI and bridge contract (controlled IPC, separate native proof)',
    async () => {
      let failure
      let cleanupFailure
      let result
      let backendTerminalCapability
      const ownedSessions = []
      const area = join(workspace, `terminal-ui-${randomUUID()}`)
      const cwdA = join(area, 'a')
      const cwdB = join(area, 'b')
      try {
        await mkdir(cwdA, { recursive: true })
        await mkdir(cwdB)
        for (const [name, cwd] of [
          ['Terminal contract A', cwdA],
          ['Terminal contract B', cwdB],
        ]) {
          const session = await json('/api/sessions', 'POST', { name, cwd })
          assert.equal(typeof session.id, 'string')
          ownedSessions.push(session.id)
        }
        await installInitialPreferences(page, ownedSessions[0], base)
        // 仅受控 IPC 正例模拟桌面可用能力；真实后端开关另由 Root 验收启用。
        await page.route('**/api/runtime/capabilities', async (route) => {
          if (new URL(route.request().url()).origin !== new URL(base).origin)
            return route.fallback()
          const response = await route.fetch({ maxRedirects: 0 })
          const capabilities = await response.json()
          assert.equal(typeof capabilities.features?.terminal, 'boolean')
          backendTerminalCapability = capabilities.features.terminal
          await route.fulfill({
            response,
            json: { ...capabilities, features: { ...capabilities.features, terminal: true } },
          })
        })
        await page.goto(new URL('/chat', base).href, { waitUntil: 'domcontentloaded' })
        await page.waitForFunction(
          () => typeof window.pisperDesktop?.terminalProfiles === 'function',
        )
        await waitForStartup(page, ['capabilities-loaded', 'sessions-loaded', 'chat-shell-ready'])
        await selectSession(page, ownedSessions[0])
        await page
          .locator('button[aria-label]:visible')
          .filter({ has: page.locator('svg.lucide-square-terminal, svg.lucide-terminal-square') })
          .first()
          .click()
        const panel = page.locator('.terminal-panel')
        await panel.waitFor({ state: 'visible', timeout: 10_000 })
        await panel.locator('.terminal-empty:not([disabled])').click()
        await panel.locator('.terminal-tab i[data-status="running"]').waitFor({ state: 'visible' })
        await panel.locator('.terminal-host.active .xterm').waitFor({ state: 'visible' })
        await page.waitForFunction(() =>
          document
            .querySelector('.terminal-host.active .xterm-rows')
            ?.textContent?.includes('bridge-output-中'),
        )
        const first = await page.evaluate(() =>
          window.__terminalContract.calls.find(
            (call) => call.command === 'desktop_terminal_create',
          ),
        )
        assert.equal(first.cwd, cwdA, 'Session A must resolve its real synthetic working directory')
        await panel.locator('.terminal-host.active .xterm-helper-textarea').focus()
        await page.keyboard.type('terminal-ui-input')
        await page.waitForFunction(() => {
          const writes = window.__terminalContract.calls.filter(
            (call) => call.command === 'desktop_terminal_write',
          )
          const bytes = writes.flatMap((call) => call.data)
          return new TextDecoder().decode(Uint8Array.from(bytes)).includes('terminal-ui-input')
        })
        await selectSession(page, ownedSessions[1])
        await panel.locator('.terminal-empty:not([disabled])').waitFor({ state: 'visible' })
        assert.equal(
          await page.evaluate(
            () =>
              window.__terminalContract.calls.filter(
                (call) => call.command === 'desktop_terminal_close',
              ).length,
          ),
          0,
          'Switching sessions must keep the hidden process alive',
        )
        await panel.locator('.terminal-empty:not([disabled])').click()
        await panel.locator('.terminal-tab i[data-status="running"]').waitFor({ state: 'visible' })
        const second = await page.evaluate(
          () =>
            window.__terminalContract.calls.filter(
              (call) => call.command === 'desktop_terminal_create',
            )[1],
        )
        assert.equal(second.cwd, cwdB, 'Session B must use its own real working directory')
        assert.notEqual(first.terminalId, second.terminalId)
        await selectSession(page, ownedSessions[0])
        await panel
          .locator(`.terminal-tab[title=${JSON.stringify(cwdA)}]`)
          .waitFor({ state: 'visible' })
        assert.equal(
          await panel.locator('.terminal-tab').count(),
          1,
          'Session B must not appear in A terminal tabs',
        )
        await panel.locator('.terminal-host.active .xterm').evaluate((element) => {
          element.dataset.contractIdentity = 'retained-a'
        })
        const beforeHide = await page.evaluate(
          () =>
            window.__terminalContract.calls.filter(
              (call) => call.command === 'desktop_terminal_create',
            ).length,
        )
        await panel.locator('.terminal-title').click()
        await panel.waitFor({ state: 'hidden' })
        await page.evaluate(() => window.dispatchEvent(new Event('pisper:toggle-terminal')))
        await panel.waitFor({ state: 'visible' })
        await panel.locator('.terminal-host.active .xterm').waitFor({ state: 'visible' })
        assert.equal(
          await panel
            .locator('.terminal-host.active .xterm')
            .getAttribute('data-contract-identity'),
          'retained-a',
          'Reopening must reattach the same xterm instance',
        )
        assert.equal(
          await page.evaluate(
            () =>
              window.__terminalContract.calls.filter(
                (call) => call.command === 'desktop_terminal_create',
              ).length,
          ),
          beforeHide,
          'Hiding/reopening must retain the existing terminal',
        )
        assert.equal(
          await page.evaluate(
            () =>
              window.__terminalContract.calls.filter(
                (call) => call.command === 'desktop_terminal_close',
              ).length,
          ),
          0,
          'Hiding the pane must keep its process alive',
        )

        // Events intentionally arrive before create acknowledgements.
        await page.evaluate(() => {
          window.__terminalContract.nextCreateMode = 'early-exit'
        })
        await newTerminalButton(panel, page).click()
        await panel.locator('.terminal-tab i[data-status="exited"]').waitFor({ state: 'visible' })
        await page.waitForFunction(() => window.__terminalContract.acknowledged === 3)
        assert.equal(
          await panel.locator('.terminal-tab i[data-status="running"]').count(),
          1,
          'Create acknowledgement must not overwrite an early Exit',
        )
        await panel.locator('.terminal-tab.active svg[aria-label]').click()
        await panel.locator('.terminal-tab i[data-status="exited"]').waitFor({ state: 'detached' })
        await page.evaluate(() => {
          window.__terminalContract.nextCreateMode = 'early-error'
        })
        await newTerminalButton(panel, page).click()
        await panel.locator('.terminal-tab i[data-status="error"]').waitFor({ state: 'visible' })
        await page.waitForFunction(() => window.__terminalContract.acknowledged === 4)
        assert.equal(
          await panel.locator('.terminal-tab i[data-status="running"]').count(),
          1,
          'Create acknowledgement must preserve early Error',
        )
        await panel.locator('.terminal-tab.active svg[aria-label]').click()
        await panel.locator('.terminal-tab i[data-status="error"]').waitFor({ state: 'detached' })

        await page.evaluate(() => {
          window.__terminalContract.nextCreateMode = 'hold'
        })
        await newTerminalButton(panel, page).click()
        await panel.locator('.terminal-tab i[data-status="starting"]').waitFor({ state: 'visible' })
        // Starting 先于异步 cwd 解析；确认 IPC 已登记后再测试关闭未确认的创建。
        await page.waitForFunction(() => window.__terminalContract.heldIds().length === 1)
        const heldId = await page.evaluate(() => window.__terminalContract.heldIds()[0])
        assert.equal(typeof heldId, 'string')
        await panel.locator('.terminal-tab.active svg[aria-label]').click()
        await panel
          .locator('.terminal-tab i[data-status="starting"]')
          .waitFor({ state: 'detached' })
        await page.evaluate((id) => window.__terminalContract.releaseCreate(id), heldId)
        await page.waitForFunction(
          (id) =>
            window.__terminalContract.calls.filter(
              (call) => call.command === 'desktop_terminal_close' && call.terminalId === id,
            ).length === 2,
          heldId,
        )
        await page.evaluate(
          (id) => window.__terminalContract.emitOutput(id, 'late-output-must-stay-hidden'),
          heldId,
        )
        await page.evaluate(
          () =>
            new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))),
        )
        assert.equal(
          await panel.locator('.terminal-tab').count(),
          1,
          'A late acknowledgement must not resurrect its closed tab',
        )
        assert.ok(!(await panel.textContent()).includes('late-output-must-stay-hidden'))
        assert.ok(
          !(await panel.locator('.xterm-rows').allTextContents())
            .join('')
            .includes('late-output-must-stay-hidden'),
        )

        await panel.locator('.terminal-tab.active svg[aria-label]').click()
        await panel.locator('.terminal-empty').waitFor({ state: 'visible' })
        await selectSession(page, ownedSessions[1])
        await panel.locator('.terminal-tab[title]').waitFor({ state: 'visible' })
        assert.equal(await panel.locator('.terminal-tab').count(), 1)
        await panel.locator('.terminal-tab.active svg[aria-label]').click()
        await panel.locator('.terminal-empty').waitFor({ state: 'visible' })
        assert.equal(
          await page.evaluate(() => window.__terminalContract.activeIds().length),
          0,
          'Explicit close must clean all owned transport terminals',
        )
        const calls = await page.evaluate(() => window.__terminalContract.calls)
        const creates = calls.filter((call) => call.command === 'desktop_terminal_create')
        assert.equal(creates.length, 5)
        for (const create of creates) {
          assert.equal(create.profileId, 'controlled-bridge-profile')
          assert.equal(typeof create.cwd, 'string')
          assert.ok(create.cols >= 2 && create.rows >= 2)
        }
        assert.ok(calls.some((call) => call.command === 'desktop_terminal_resize'))
        result = {
          layer: 'frontend-bridge-contract',
          productionBridge: true,
          controlledIpc: true,
          backendTerminalCapability,
          controlledUiTerminalCapability: true,
          creates: creates.length,
          installedWebviewTested: false,
        }
      } catch (error) {
        failure = error
      } finally {
        const errors = []
        for (const cleanup of [
          () => page.evaluate(() => window.__terminalContract?.releaseAll?.()),
          () => page.evaluate(() => window.pisperDesktop?.terminalCloseAll?.()),
          ...ownedSessions.map(
            (id) => () => json(`/api/sessions/${encodeURIComponent(id)}`, 'DELETE'),
          ),
          () => rmdir(cwdB),
          () => rmdir(cwdA),
          () => rmdir(area),
        ]) {
          try {
            await cleanup()
          } catch (error) {
            errors.push(error)
          }
        }
        if (errors.length)
          cleanupFailure = new AggregateError(errors, 'Owned terminal fixture cleanup failed')
      }
      if (failure && cleanupFailure)
        throw new AggregateError(
          [failure, cleanupFailure],
          'Terminal assertion and owned IPC cleanup failed',
        )
      if (failure) throw failure
      if (cleanupFailure) throw cleanupFailure
      return result
    },
  )
}

function installControlledTransport() {
  if (window.top !== window) return
  if (window.__TAURI_INTERNALS__)
    throw new Error('Refusing to replace an existing native Tauri transport')
  let callbackId = 0
  const callbacks = new Map()
  const channels = new Map()
  const archivedChannels = new Map()
  const pendingCreates = new Map()
  const evidence = { calls: [], acknowledged: 0, nextCreateMode: 'ordinary', activeSessionId: '' }
  window.__terminalContract = evidence
  localStorage.setItem('pisper:startup-diagnostics', '1')
  window.addEventListener('pisper:active-session-changed', (event) => {
    evidence.activeSessionId = event.detail?.id || ''
  })
  const emit = (id, message) => {
    const channel = archivedChannels.get(id)
    if (channel) callbacks.get(channel.callback)?.({ index: channel.index++, message })
  }
  evidence.emitOutput = (id, text) => {
    const bytes = new TextEncoder().encode(text)
    const marker = new TextEncoder().encode('中')
    const split = bytes.findIndex((byte) => byte === marker[0])
    const pieces = split >= 0 ? [bytes.slice(0, split + 1), bytes.slice(split + 1)] : [bytes]
    for (const piece of pieces)
      emit(id, { type: 'output', terminalId: id, data: Array.from(piece) })
  }
  evidence.heldIds = () => [...pendingCreates.keys()]
  evidence.activeIds = () => [...channels.keys()]
  evidence.releaseCreate = (id) => {
    const release = pendingCreates.get(id)
    if (!release) throw new Error('Unknown owned pending terminal create')
    pendingCreates.delete(id)
    release()
  }
  evidence.releaseAll = () => {
    for (const id of [...pendingCreates.keys()]) evidence.releaseCreate(id)
  }
  window.__TAURI_INTERNALS__ = {
    transformCallback: (callback) => {
      const id = ++callbackId
      callbacks.set(id, callback)
      return id
    },
    unregisterCallback: (id) => callbacks.delete(id),
    invoke: async (command, args = {}) => {
      if (command.startsWith('desktop_terminal_')) {
        evidence.calls.push({
          command,
          ...(args.input || {}),
          ...(args.data ? { data: args.data } : {}),
          ...(args.terminalId ? { terminalId: args.terminalId } : {}),
        })
      }
      if (command === 'desktop_terminal_profiles') {
        return [
          { id: 'controlled-bridge-profile', label: 'Controlled bridge fixture', default: true },
        ]
      }
      if (command === 'desktop_terminal_create') {
        const input = args.input
        const mode = evidence.nextCreateMode
        evidence.nextCreateMode = 'ordinary'
        const channel = { callback: args.onEvent.id, index: 0 }
        channels.set(input.terminalId, channel)
        archivedChannels.set(input.terminalId, channel)
        evidence.emitOutput(input.terminalId, 'Controlled IPC bridge-output-中\r\n')
        if (mode === 'early-exit')
          emit(input.terminalId, { type: 'exit', terminalId: input.terminalId, code: 7 })
        if (mode === 'early-error')
          emit(input.terminalId, {
            type: 'error',
            terminalId: input.terminalId,
            message: 'Controlled IPC early error',
          })
        if (mode === 'hold')
          await new Promise((resolve) => pendingCreates.set(input.terminalId, resolve))
        await Promise.resolve()
        evidence.acknowledged += 1
        return { terminalId: input.terminalId, profileId: input.profileId, cwd: input.cwd }
      }
      if (command === 'desktop_terminal_write' || command === 'desktop_terminal_resize') return
      if (command === 'desktop_terminal_close') return channels.delete(args.terminalId)
      if (command === 'desktop_terminal_close_all') {
        const count = channels.size
        channels.clear()
        return count
      }
      if (command === 'desktop_get_app_info') {
        return {
          desktop: true,
          packaged: false,
          version: 'fixture',
          platform: 'windows',
          arch: 'x64',
          releasesUrl: '',
        }
      }
      if (command === 'desktop_set_language') return args.language
      if (command === 'desktop_remote_list' || command === 'desktop_component_update_status')
        return []
      if (command === 'desktop_pet_sync_menu' || command === 'desktop_pet_apply_enabled') return
      throw new Error('Desktop command is outside the isolated terminal contract fixture')
    },
  }
}

function newTerminalButton(panel, page) {
  return panel.locator('button[aria-label]').filter({ has: page.locator('svg.lucide-plus') })
}

async function waitForStartup(page, phases) {
  await page.waitForFunction(
    (phases) => phases.every((phase) => performance.getEntriesByName(`pisper-${phase}`).length),
    phases,
    { timeout: 10_000 },
  )
}

async function selectSession(page, id) {
  await page.evaluate(
    (sessionId) =>
      window.dispatchEvent(
        new CustomEvent('pisper:session-selected', { detail: { sessionId, disposition: 'open' } }),
      ),
    id,
  )
  await page.waitForFunction((id) => window.__terminalContract.activeSessionId === id, id, {
    timeout: 10_000,
  })
}

async function installInitialPreferences(page, id, base) {
  await page.route('**/api/local/browser-preferences', async (route) => {
    if (new URL(route.request().url()).origin !== new URL(base).origin) return route.fallback()
    if (route.request().method() !== 'GET') return route.continue()
    const response = await route.fetch({ maxRedirects: 0 })
    const snapshot = await response.json()
    await route.fulfill({
      response,
      json: {
        ...snapshot,
        values: {
          ...snapshot.values,
          'pisper-active-session': id,
          'pisper-terminal-panel': JSON.stringify({ open: false, height: 300 }),
        },
      },
    })
  })
}

// Factory contract: async ({name, mobile}) => {page, dispose: async()=>...}.
// Every call must create a fresh owned context with the synthetic backend cookie
// and Root's network restrictions. No installed/native WebView is driven here.
export async function checkTerminalEntrypointNegatives({ check, base, createIsolatedPage }) {
  assert.equal(typeof createIsolatedPage, 'function')
  for (const mobile of [false, true]) {
    await check(
      `${mobile ? 'Mobile app' : 'Web'} terminal entry remains absent without desktop IPC (UI contract)`,
      async () => {
        const owned = await createIsolatedPage({
          name: mobile ? 'terminal-mobile-negative' : 'terminal-web-negative',
          mobile,
        })
        let failure
        let cleanupFailure
        try {
          await owned.page.addInitScript((mobile) => {
            if (window.top !== window) return
            localStorage.setItem('pisper:startup-diagnostics', '1')
            if (mobile) window.__PISPER_MOBILE_APP__ = true
          }, mobile)
          await owned.page.route('**/api/runtime/capabilities', async (route) => {
            if (new URL(route.request().url()).origin !== new URL(base).origin)
              return route.fallback()
            const response = await route.fetch({ maxRedirects: 0 })
            const capabilities = await response.json()
            await route.fulfill({
              response,
              json: {
                ...capabilities,
                profile: mobile ? 'mobile-embedded' : 'desktop',
                features: { ...capabilities.features, terminal: !mobile },
              },
            })
          })
          await owned.page.goto(new URL('/chat', base).href, { waitUntil: 'domcontentloaded' })
          await waitForStartup(owned.page, [
            'capabilities-loaded',
            'client-info-loaded',
            'react-app-mounted',
            'chat-shell-ready',
          ])
          assert.equal(
            await owned.page.evaluate(() => typeof window.pisperDesktop?.terminalProfiles),
            'undefined',
          )
          await owned.page.evaluate(() => window.dispatchEvent(new Event('pisper:toggle-terminal')))
          assert.equal(await owned.page.locator('.terminal-panel').count(), 0)
          assert.equal(
            await owned.page
              .locator('button svg.lucide-square-terminal, button svg.lucide-terminal-square')
              .count(),
            0,
            'No terminal toggle button may appear without desktop support',
          )
        } catch (error) {
          failure = error
        } finally {
          try {
            await disposeOwnedPage(owned)
          } catch (error) {
            cleanupFailure = error
          }
        }
        if (failure && cleanupFailure)
          throw new AggregateError(
            [failure, cleanupFailure],
            'Terminal negative assertion and owned context cleanup failed',
          )
        if (failure) throw failure
        if (cleanupFailure) throw cleanupFailure
        return {
          layer: 'frontend-entry-contract',
          client: mobile ? 'mobile-app' : 'web',
          terminalVisible: false,
          installedWebviewTested: false,
        }
      },
    )
  }
  await check(
    'Unsupported desktop capability keeps the terminal entry absent with production bridge (controlled IPC)',
    async () => {
      let owned
      let failure
      const cleanupFailures = []
      try {
        owned = await createIsolatedPage({ name: 'terminal-capability-negative', mobile: false })
        const bridge = await readFile(
          new URL('../src-tauri/src/desktop_shell/desktop-bridge.js', import.meta.url),
          'utf8',
        )
        await owned.page.addInitScript({
          content: `;(${installControlledTransport.toString()})();\n${bridge}`,
        })
        await owned.page.route('**/api/runtime/capabilities', async (route) => {
          if (new URL(route.request().url()).origin !== new URL(base).origin)
            return route.fallback()
          const response = await route.fetch({ maxRedirects: 0 })
          const capabilities = await response.json()
          await route.fulfill({
            response,
            json: { ...capabilities, features: { ...capabilities.features, terminal: false } },
          })
        })
        await owned.page.goto(new URL('/chat', base).href, { waitUntil: 'domcontentloaded' })
        await waitForStartup(owned.page, [
          'capabilities-loaded',
          'client-info-loaded',
          'chat-shell-ready',
        ])
        assert.equal(
          await owned.page.evaluate(() => typeof window.pisperDesktop?.terminalProfiles),
          'function',
          'The production desktop bridge must be available in this negative case',
        )
        await owned.page.evaluate(() => window.dispatchEvent(new Event('pisper:toggle-terminal')))
        await owned.page.evaluate(
          () =>
            new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))),
        )
        const calls = await owned.page.evaluate(() => window.__terminalContract.calls)
        assert.equal(
          calls.length,
          0,
          'Unsupported capability must prevent even terminal profile discovery and creation',
        )
        assert.equal(
          await owned.page.locator('.terminal-panel').count(),
          0,
          'The capability gate must keep the terminal entry disabled even with a desktop bridge',
        )
        assert.equal(
          await owned.page
            .locator('button svg.lucide-square-terminal, button svg.lucide-terminal-square')
            .count(),
          0,
          'The capability gate must also suppress the terminal toggle button',
        )
      } catch (error) {
        failure = error
      } finally {
        for (const cleanup of [() => owned && disposeOwnedPage(owned)]) {
          try {
            await cleanup()
          } catch (error) {
            cleanupFailures.push(error)
          }
        }
      }
      if (failure && cleanupFailures.length)
        throw new AggregateError(
          [failure, ...cleanupFailures],
          'Desktop capability assertion and cleanup failed',
        )
      if (failure) throw failure
      if (cleanupFailures.length)
        throw new AggregateError(cleanupFailures, 'Desktop capability context cleanup failed')
      return {
        layer: 'frontend-entry-contract',
        controlledIpc: true,
        terminalVisible: false,
        installedWebviewTested: false,
      }
    },
  )
}

async function disposeOwnedPage(owned) {
  if (owned.dispose) return owned.dispose()
  assert.ok(owned.context, 'The factory must return dispose() or its owned context')
  await owned.context.close()
}

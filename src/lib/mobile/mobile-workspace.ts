type MobileWorkspaceState = {
  mode: 'local' | 'remote' | null
  onDevice?: { running: boolean }
}

function invokeMobile<T>(command: string): Promise<T> {
  const invoke = window.__TAURI__?.core?.invoke ?? window.__TAURI_INTERNALS__?.invoke
  if (!invoke) return Promise.reject(new Error('native bridge unavailable'))
  return invoke<T>(command)
}

export async function mobileWorkspaceMode(): Promise<'local' | 'remote' | null> {
  if (!window.__PISPER_MOBILE_APP__) return null
  const state = await invokeMobile<MobileWorkspaceState>('mobile_state')
  return state.mode
}

export async function importMobileWorkspaceDirectory(): Promise<string | null> {
  if (window.__PISPER_MOBILE_PLATFORM__ !== 'android') {
    throw new Error('workspace_import_unsupported')
  }
  // 目录只导入本机工作区；远程模式绝不能收到手机路径或 content URI。
  const state = await invokeMobile<MobileWorkspaceState>('mobile_state')
  if (state.mode !== 'local' || !state.onDevice?.running) {
    throw new Error('workspace_import_requires_local_runtime')
  }
  const result = await invokeMobile<{ path: string | null }>('mobile_import_workspace_directory')
  if (result.path === null) return null
  if (typeof result.path !== 'string' || !result.path.startsWith('/')) {
    throw new Error('workspace_import_invalid_path')
  }
  return result.path
}

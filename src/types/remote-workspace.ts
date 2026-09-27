// 桌面原生层的公开连接契约，不包含设备令牌或本机 Runtime 凭据。
export type RemoteWorkspaceSummary = {
  id: string
  name: string
  address: string
  fingerprint: string
  connected: boolean
}

export type RemoteWorkspaceInput = {
  name: string
  address: string
  fingerprint: string
  code: string
}

export type RemoteWorkspaceBridge = {
  list: () => Promise<RemoteWorkspaceSummary[]>
  pair: (input: RemoteWorkspaceInput) => Promise<string>
  open: (id: string) => Promise<void>
  forget: (id: string) => Promise<void>
}

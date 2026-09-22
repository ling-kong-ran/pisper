// 客户端表单与 Runtime 共用网关元数据；不加载 SDK 或启动连接。

export const DECISION_PROVIDER_CATALOG = Object.freeze({
  // TypeSafe 官方接口。文档：https://docs.typesafe.ai（early access，需要控制台发放权限）
  typesafe: Object.freeze({
    protocol: 'typesafe-decisions',
    endpointSuffixes: Object.freeze(['/systemone', '/decisions']),
    defaultBaseUrl: 'https://api.typesafe.ai',
    path: '/v1/systemone',
    defaultModelId: 'jev-1.13.0',
  }),
  // OpenRouter 的 decisions 是独立协议，不在 /v1 之下（/v1/alpha/decisions 会 404）。
  openrouter: Object.freeze({
    protocol: 'typesafe-decisions',
    endpointSuffixes: Object.freeze(['/systemone', '/decisions']),
    defaultBaseUrl: 'https://openrouter.ai/api',
    path: '/alpha/decisions',
    defaultModelId: 'typesafe/jev-1.13',
  }),
  // 自定义/中转：用户自填 baseUrl，协议路径按 openrouter 形态（/alpha/decisions）拼接，
  // 也允许 baseUrl 直接写完整接口地址（以 /systemone 或 /decisions 结尾时原样使用）。
  custom: Object.freeze({
    protocol: 'typesafe-decisions',
    endpointSuffixes: Object.freeze(['/systemone', '/decisions']),
    defaultBaseUrl: '',
    path: '/alpha/decisions',
    defaultModelId: 'typesafe/jev-1.13',
  }),
})

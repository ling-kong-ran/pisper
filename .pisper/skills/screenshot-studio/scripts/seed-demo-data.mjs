// Seed fictional demo data for screenshot capture through the real runtime APIs.
// Creates: 6 sessions (3 with injected transcripts, 3 empty with minimal JSONL),
// link + generated-image assets, a memory space with nodes, schedules, workflows,
// and a configured OpenAI-compatible provider. Writes generated/screenshot-run/state.json
// with the created ids so capture-screenshots.mjs can resolve them.
import { mkdirSync, writeFileSync } from 'node:fs'
import { deflateSync } from 'node:zlib'
import { resolve } from 'node:path'
import { AGENT_DIR, BASE_URL, RUN_DIR, WORKSPACE_DIR } from './screenshot-config.mjs'

const SESSION_DIR = resolve(AGENT_DIR, 'sessions')
const STATE_PATH = resolve(RUN_DIR, 'state.json')
const CWD = WORKSPACE_DIR
const DISPLAY_CWD = '.'

const NAMES = [
  '调研向量数据库选型',
  '优化 React 首屏加载',
  '整理本周发布说明',
  '解析合同关键条款',
  '排查 CI 构建失败',
  '设计新版登录页',
]

async function api(path, { method = 'GET', body } = {}) {
  const res = await fetch(`${BASE_URL}${path}`, {
    method,
    headers: body ? { 'Content-Type': 'application/json' } : undefined,
    body: body ? JSON.stringify(body) : undefined,
  })
  if (!res.ok) {
    const text = await res.text()
    throw new Error(`${method} ${path} -> ${res.status}: ${text.slice(0, 300)}`)
  }
  return res.json()
}

// ---------- tiny PNG encoder (pure node, RGB) ----------
function crc32(buf) {
  const t = []
  for (let n = 0; n < 256; n++) {
    let c = n
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1
    t[n] = c >>> 0
  }
  let crc = 0xffffffff
  for (const b of buf) crc = t[(crc ^ b) & 0xff] ^ (crc >>> 8)
  return (crc ^ 0xffffffff) >>> 0
}
function pngChunk(type, data) {
  const len = Buffer.alloc(4)
  len.writeUInt32BE(data.length)
  const t = Buffer.from(type, 'ascii')
  const crc = Buffer.alloc(4)
  crc.writeUInt32BE(crc32(Buffer.concat([t, data])))
  return Buffer.concat([len, t, data, crc])
}
function makePng(width, height, rowFn) {
  const sig = Buffer.from([137, 80, 78, 71, 13, 10, 26, 10])
  const ihdr = Buffer.alloc(13)
  ihdr.writeUInt32BE(width, 0)
  ihdr.writeUInt32BE(height, 4)
  ihdr[8] = 8
  ihdr[9] = 2
  const raw = Buffer.alloc(height * (1 + width * 3))
  for (let y = 0; y < height; y++) {
    raw[y * (1 + width * 3)] = 0
    for (let x = 0; x < width; x++) {
      const [r, g, b] = rowFn(x, y)
      const o = y * (1 + width * 3) + 1 + x * 3
      raw[o] = r
      raw[o + 1] = g
      raw[o + 2] = b
    }
  }
  return Buffer.concat([
    sig,
    pngChunk('IHDR', ihdr),
    pngChunk('IDAT', deflateSync(raw)),
    pngChunk('IEND', Buffer.alloc(0)),
  ])
}
const mockPng = makePng(640, 400, (x, y) => {
  const t = y / 400
  const band = Math.sin(x / 26) * 14 * (1 - t)
  return [
    Math.round(14 + (99 - 14) * t + band),
    Math.round(165 + (102 - 165) * t + band),
    Math.round(233 + (241 - 233) * t),
  ]
})
const chartPng = makePng(640, 400, (x, y) => {
  const lineY = 340 - (Math.sin(x / 47) * 60 + x * 0.32)
  return Math.abs(y - lineY) < 4 ? [52, 211, 153] : [15, 23, 42]
})

// ---------- sessions (created through the API so ids come from the runtime) ----------
const sessions = []
for (const name of NAMES) {
  const created = await api('/api/sessions', {
    method: 'POST',
    body: { name, cwd: CWD },
  })
  sessions.push(created.id)
}
for (const [index, id] of sessions.entries()) {
  await api(`/api/sessions/${id}`, { method: 'PATCH', body: { name: NAMES[index] } })
  await api(`/api/sessions/${id}/cwd`, { method: 'PUT', body: { cwd: CWD } })
}
console.log('sessions created:', sessions.map((id) => id.slice(0, 8)).join(', '))

// ---------- transcripts ----------
function hex8() {
  return Math.floor(Math.random() * 0xffffffff)
    .toString(16)
    .padStart(8, '0')
}
function iso(ms) {
  return new Date(ms).toISOString()
}
function fileName(ms, id) {
  return `${iso(ms).replace(/[:.]/g, '-')}_${id}.jsonl`
}
function writeTranscript(sessionId, name, pairs, { branches = [], labels = [] } = {}) {
  mkdirSync(SESSION_DIR, { recursive: true })
  const baseMs = Date.now() - 1000 * 60 * 60 * 6
  const events = []
  let prev = null
  events.push({
    type: 'session',
    version: 3,
    id: sessionId,
    timestamp: iso(baseMs),
    cwd: CWD.replace(/\\/g, '\\\\'),
  })
  const infoId = hex8()
  events.push({ type: 'session_info', id: infoId, parentId: null, timestamp: iso(baseMs), name })
  prev = infoId
  const modelId = hex8()
  events.push({
    type: 'model_change',
    id: modelId,
    parentId: prev,
    timestamp: iso(baseMs + 400),
    provider: 'openai',
    modelId: 'gpt-5',
  })
  prev = modelId
  let t = baseMs + 1000
  const turns = []
  for (const [userText, assistantBlocks] of pairs) {
    const userEventId = hex8()
    events.push({
      type: 'message',
      id: userEventId,
      parentId: prev,
      timestamp: iso(t),
      message: { role: 'user', content: [{ type: 'text', text: userText }], timestamp: t },
    })
    prev = userEventId
    t += 9000
    const assistantId = hex8()
    const content = assistantBlocks.map((block) =>
      block.type === 'thinking'
        ? {
            type: 'thinking',
            thinking: block.thinking,
            thinkingSignature: JSON.stringify({
              summary: [{ type: 'summary_text', text: block.thinking }],
            }),
          }
        : {
            type: 'text',
            text: block.text,
            textSignature: JSON.stringify({ v: 1, id: `msg_${hex8()}`, phase: 'commentary' }),
          },
    )
    events.push({
      type: 'message',
      id: assistantId,
      parentId: prev,
      timestamp: iso(t),
      message: { role: 'assistant', content, stopReason: 'stop', timestamp: t },
    })
    turns.push({ userId: userEventId, assistantId })
    prev = assistantId
    t += 8000
  }

  const branchPoint = turns[0]?.assistantId
  for (const [userText, assistantText] of branches) {
    if (!branchPoint) break
    const userId = hex8()
    events.push({
      type: 'message',
      id: userId,
      parentId: branchPoint,
      timestamp: iso(t),
      message: { role: 'user', content: [{ type: 'text', text: userText }], timestamp: t },
    })
    t += 7000
    const assistantId = hex8()
    events.push({
      type: 'message',
      id: assistantId,
      parentId: userId,
      timestamp: iso(t),
      message: {
        role: 'assistant',
        content: [{ type: 'text', text: assistantText }],
        stopReason: 'stop',
        timestamp: t,
      },
    })
    turns.push({ userId, assistantId })
    prev = assistantId
    t += 7000
  }

  for (const [turnIndex, label] of labels) {
    const targetId = turns[turnIndex]?.assistantId
    if (!targetId) continue
    const labelId = hex8()
    events.push({
      type: 'label',
      id: labelId,
      parentId: prev,
      timestamp: iso(t),
      targetId,
      label,
    })
    prev = labelId
    t += 1000
  }
  writeFileSync(
    resolve(SESSION_DIR, fileName(baseMs, sessionId)),
    events.map((e) => JSON.stringify(e)).join('\n') + '\n',
  )
  console.log('transcript', sessionId.slice(0, 8), name)
}

// Minimal JSONL so empty demo sessions survive a server restart.
function writeEmptyTranscript(sessionId, name) {
  mkdirSync(SESSION_DIR, { recursive: true })
  const ms = Date.now() - 1000 * 60 * 60 * 5
  const events = [
    {
      type: 'session',
      version: 3,
      id: sessionId,
      timestamp: iso(ms),
      cwd: CWD.replace(/\\/g, '\\\\'),
    },
    { type: 'session_info', id: hex8(), parentId: null, timestamp: iso(ms), name },
  ]
  writeFileSync(
    resolve(SESSION_DIR, fileName(ms, sessionId)),
    events.map((e) => JSON.stringify(e)).join('\n') + '\n',
  )
  console.log('empty transcript', sessionId.slice(0, 8), name)
}

writeTranscript(sessions[0], NAMES[0], [
  [
    '帮我在 Pinecone、Qdrant 和 Milvus 之间做一次向量数据库选型对比，重点关注自托管成本、检索延迟和生态成熟度。',
    [
      { type: 'thinking', thinking: '**Planning vector database comparison**\n需要一份可落地的选型对比。先明确对比维度，再给出结论与迁移建议。' },
      {
        type: 'text',
        text: '以下是三者的横向对比，结论放在最后：\n\n| 维度 | Pinecone | Qdrant | Milvus |\n| --- | --- | --- | --- |\n| 自托管成本 | 托管为主，自托管较少 | 轻量，单机即可 | 组件多，运维重 |\n| 检索延迟 | 低（托管） | 低 | 中 |\n| 生态成熟度 | 高 | 高 | 高（中文资料多） |\n\n**建议**：团队小、追求低运维选 **Qdrant**；已有 Kubernetes 且需要高吞吐写入选 **Milvus**；不想运维直接选 **Pinecone 托管**。',
      },
    ],
  ],
  [
    '如果数据量在千万级向量，Qdrant 单机够用吗？',
    [
      { type: 'thinking', thinking: '**Capacity check**\n千万级向量需要估算内存占用，按 768 维 float32 计算。' },
      {
        type: 'text',
        text: '可以，但要看内存预算。768 维 float32 下，1 千万向量原始数据约 **30 GB**，加上索引（HNSW M=16 约 1.2 倍）建议 **64 GB 内存**起步。若用 int8 或 PQ 量化可降到 8–12 GB。\n\n稳妥起见先在测试环境压测 Recall@10，再决定是否上集群。',
      },
    ],
  ],
], {
  branches: [
    [
      '如果优先考虑完全离线与数据主权，结论会变化吗？',
      '会。把“数据不出内网”设为硬约束后，优先级应调整为 **Qdrant > Milvus > Pinecone**。Qdrant 的单节点部署和备份链路更短，适合先小规模上线；当写入吞吐和多租户隔离成为主要矛盾时，再评估迁移到 Milvus 集群。',
    ],
    [
      '先给我一个两周内可以完成的验证计划。',
      '建议拆成两个阶段：\n\n1. **第 1 周：基线验证**：导入 100 万条脱敏向量，记录 P50/P95 延迟、Recall@10、索引时间和峰值内存。\n2. **第 2 周：故障与扩容**：演练备份恢复、滚动升级和容量翻倍，最终用同一份评分表评审。\n\n验收门槛可以设为 P95 小于 80 ms、Recall@10 大于 0.92、恢复时间小于 30 分钟。',
    ],
  ],
  labels: [
    [0, '选型基线'],
    [1, '千万级容量'],
    [2, '离线部署'],
    [3, '两周验证计划'],
  ],
})

writeTranscript(sessions[1], NAMES[1], [
  [
    '我们站点的首屏加载太慢，LCP 在 4 秒以上，帮我定位并给出优化方案。',
    [
      { type: 'thinking', thinking: '**Diagnosing LCP**\n先看资源加载链路：入口 JS 体积、图片、字体与第三方脚本。' },
      {
        type: 'text',
        text: '常见瓶颈和对应方案：\n\n1. **入口包过大** —— 用 `React.lazy` 拆分路由级 chunk，把首屏依赖收敛到 200 KB 以内。\n2. **字体阻塞渲染** —— 加 `font-display: swap` 并预加载关键字体。\n3. **图片未优化** —— 转 WebP/AVIF，配合 `loading="lazy"` 与 `srcset`。\n4. **第三方脚本** —— 延迟到 `requestIdleCallback` 再加载。\n\n建议先跑一次 Lighthouse 拿到瓶颈分布，我可以继续帮你落地具体改动。',
      },
    ],
  ],
  [
    '帮我看看现在的 bundle 里最大的几个 chunk 是什么。',
    [
      { type: 'thinking', thinking: '**Analyzing bundle**\n读取构建产物并排序 chunk 体积。' },
      {
        type: 'text',
        text: '从构建产物看，当前最大的三个 chunk 分别是：\n\n- `vendor-charts` ≈ 480 KB\n- `editor-kit` ≈ 320 KB\n- `main` ≈ 260 KB\n\n`vendor-charts` 只被图表页使用，建议拆成独立路由懒加载，预计可把首屏总 JS 减少约 45%。',
      },
    ],
  ],
], { labels: [[1, '两周验证计划']] })

writeTranscript(sessions[2], NAMES[2], [
  [
    '把本周的功能更新整理成一份发布说明，按用户可见程度排序。',
    [
      {
        type: 'text',
        text: '## 本周更新\n\n**全新体验**\n- 会话页重构：消息正文加宽到 900px，Agent 回复无框化，thinking 与工具过程可折叠。\n- Composer 整合运行配置：目录、附件、模型、Git/SVN 变更集中到底部输入区。\n\n**能力增强**\n- 统一组件更新：一次检查 Desktop、TUI 与 Runtime，只下载落后组件。\n- 会话标题改为本地生成，不再消耗模型 Token。\n\n**稳定性**\n- 修复定时任务重启后遗留 running 记录的问题。\n- 修复会话 LRU 回收活动 Runtime 的问题。',
      },
    ],
  ],
], { labels: [[0, '发布说明定稿']] })

writeEmptyTranscript(sessions[3], NAMES[3])
writeEmptyTranscript(sessions[4], NAMES[4])
writeEmptyTranscript(sessions[5], NAMES[5])

// ---------- assets ----------
for (const asset of [
  { kind: 'link', name: 'Pinecone 官方文档', url: 'https://docs.pinecone.io/', sessionName: NAMES[0] },
  { kind: 'link', name: 'Qdrant 自托管安装指南', url: 'https://qdrant.tech/documentation/guides/installation/', sessionName: NAMES[0] },
  { kind: 'link', name: 'Milvus 架构说明', url: 'https://milvus.io/docs/architecture_overview.md', sessionName: NAMES[0] },
  { name: 'dashboard-mock.png', mimeType: 'image/png', data: mockPng.toString('base64'), sessionName: NAMES[0] },
  { name: 'chart-preview.png', mimeType: 'image/png', data: chartPng.toString('base64'), sessionName: NAMES[1] },
]) {
  await api('/api/assets', { method: 'POST', body: asset })
}
console.log('assets created')

// ---------- memory ----------
const space = await api('/api/memory/spaces', { method: 'POST', body: { name: '项目知识' } })
for (const node of [
  { title: '发布流程', content: '发布前必须运行 npm run check 与 npm test；Desktop、TUI、Runtime 版本相互独立。' },
  { title: '运行环境', content: '需要 Node.js 20+，桌面端基于 Tauri 与 Node SEA，TUI 使用 Ratatui。' },
  { title: '数据安全', content: '本地优先，会话默认保存在 ~/.pisper/agent，Runtime 只监听 127.0.0.1。' },
  { title: '组件更新', content: '统一更新入口只下载版本落后的组件，Desktop 与 Runtime 安装后重启一次生效。' },
]) {
  await api('/api/memory/nodes', { method: 'POST', body: { ...node, spaceId: space?.id } })
}
console.log('memory space created:', space?.id)

// ---------- schedules ----------
for (const task of [
  { name: '每日站会摘要', prompt: '汇总昨日进展并生成今日站会要点。', frequency: 'daily', time: '09:30', timezone: 'Asia/Hong_Kong', notifications: ['browser'] },
  { name: '依赖安全公告巡检', prompt: '检查依赖库是否有新的安全公告并汇总。', frequency: 'daily', time: '18:00', timezone: 'Asia/Hong_Kong' },
  { name: '每周数据备份提醒', prompt: '提醒执行本地数据备份并核对备份文件。', frequency: 'weekly', dayOfWeek: 5, time: '21:00', timezone: 'Asia/Hong_Kong' },
]) {
  await api('/api/schedules', { method: 'POST', body: { ...task, cwd: DISPLAY_CWD } })
}
console.log('schedules created')

// ---------- workflows ----------
await api('/api/workflows', {
  method: 'POST',
  body: {
    name: '每日摘要自动生成',
    description: '定时汇总渠道消息并生成摘要推送。',
    status: 'published',
    cwd: DISPLAY_CWD,
    nodes: [
      { id: 'n1', kind: 'trigger', label: '每日触发', x: 80, y: 120 },
      { id: 'n2', kind: 'prompt', label: '生成摘要', prompt: '汇总各渠道今日消息并生成 200 字摘要。', x: 340, y: 120 },
      { id: 'n3', kind: 'notification', label: '推送摘要', x: 600, y: 120 },
    ],
    edges: [
      { id: 'e1', source: 'n1', target: 'n2' },
      { id: 'e2', source: 'n2', target: 'n3' },
    ],
  },
})
await api('/api/workflows', {
  method: 'POST',
  body: {
    name: '发布检查流水线',
    description: '运行检查、构建并汇总结果。',
    status: 'draft',
    cwd: DISPLAY_CWD,
    nodes: [
      { id: 'a1', kind: 'prompt', label: '运行质量检查', prompt: '运行 npm run check 并汇总失败项。', x: 80, y: 120 },
      { id: 'a2', kind: 'prompt', label: '构建产物', prompt: '执行 npm run build 并核对 bundle 预算。', x: 360, y: 120 },
    ],
    edges: [{ id: 'b1', source: 'a1', target: 'a2' }],
  },
})
console.log('workflows created')

// ---------- provider (top-level apiKey is required) ----------
await api('/api/config', {
  method: 'PUT',
  body: {
    provider: 'openai',
    model: 'gpt-5',
    apiKey: 'sk-demo',
    defaultProvider: 'openai',
    defaultModel: 'gpt-5',
    thinkingLevel: 'medium',
    toolMode: 'full',
  },
})
console.log('provider configured')

// ---------- state for capture-screenshots.mjs ----------
const workflows = await api('/api/workflows')
const published = workflows.workflows.find((w) => w.name === '每日摘要自动生成')
const state = {
  sessions,
  conversationSessionId: sessions[0],
  splitSessionId: sessions[1],
  welcomeSessionId: sessions[3],
  memorySpaceId: space?.id,
  workflowId: published?.id || '',
  cwd: DISPLAY_CWD,
}
mkdirSync(RUN_DIR, { recursive: true })
writeFileSync(STATE_PATH, JSON.stringify(state, null, 2) + '\n')
console.log('seed done ->', STATE_PATH)

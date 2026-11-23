// 问法取自需求讨论，标为原问的三条保留原措辞（含拼写）。其余问法和所有源对话
// 都是为评测改编的场景。时间、站点和结论是虚构值，不从用户的记忆库取样。
export const turns = [
  ['mirror', 'drill', '记住，演练服的玩家镜像快照每天 03:20 运行。', '收到。'],
  [
    'search',
    'drill',
    '方案已经定稿。记忆搜索使用 SQLite FTS5，没有部署向量模型。',
    '我会按这个实现维护说明。',
  ],
  [
    'incident',
    'drill',
    '我核过两轮回放，积分重复的原因是结算日志被重放。用 round_key 去重后，两轮对账都一致。',
    '我把核对结果放进排查记录了。',
  ],
  ['anchor', 'drill', '记住，配置来源校验还没实现，文件改动后要人工复核。', '收到。'],
  ['other', 'archive', '记住，归档站的玩家镜像快照每天 21:45 运行。', '收到。'],
  ['private', 'chat', '记住，这次单独演练的回滚口令是“撤回蓝色棋子”，只在这个聊天里用。', '收到。'],
  ['preference', 'chat', '以后所有项目的排查报告都先写结论，再列证据。', '好的。'],
  ['noise', 'drill', '在吗？先帮我看一眼。', '在，请发来。'],
  ['index', 'drill', '记住，演练服的快照索引每周一重建。', '收到。'],
  ['cleanup', 'drill', '记住，演练服的快照清理保留最近七天。', '收到。'],
  ['format', 'drill', '记住，演练服的快照导出统一存为 Parquet。', '收到。'],
  ['timezone', 'drill', '记住，演练服的快照日志统一使用 UTC。', '收到。'],
  ['board', 'chat', '记住，我这个独立沙盘的棋盘边长固定为十九格。', '收到。'],
].map(([key, workspace, user, assistant]) => ({ key, workspace, user, assistant }))

// 按可回答的事实标注，独立于模型选用的标题、类型、topic 和扩展词。一轮可有多个答案。
export const answers = [
  ['schedule', 'mirror', 'drill', /0?3[:：]20/],
  ['model', 'search', 'drill', /fts5/i, /(?:没有|无|未|不).{0,12}(?:向量|embedding)/iu],
  ['cause', 'incident', 'drill', /结算日志/, /重放|回放/],
  ['remedy', 'incident', 'drill', /round_key/i, /去重/],
  ['unfinished', 'anchor', 'drill', /配置/, /(?:未|没).{0,3}实现/, /人工|手动/],
  ['otherSchedule', 'other', 'archive', /21[:：]45/],
  ['report', 'preference', 'global', /先.{0,6}结论/, /再.{0,6}证据/],
  ['reindex', 'index', 'drill', /周一|星期一/],
  ['retention', 'cleanup', 'drill', /[7七]\s*天/],
  ['export', 'format', 'drill', /parquet/i],
  ['logZone', 'timezone', 'drill', /UTC/i],
  ['boardSize', 'board', 'chat', /(?:十九|19)\s*格/],
].map(([key, source, scope, ...patterns]) => ({ key, source, scope, patterns }))

// 每个正例只有一个必需答案，所以命中正例数 / 正例数即这里的 recall@4。
export const questions = [
  ['mirror-original', 'drill', '那个镜像任务几点跑', 'schedule'],
  ['model-original', 'drill', 'embing 模型用的什么', 'model'],
  ['unfinished-original', 'drill', '还有哪些没实现', 'unfinished'],
  ['mirror-explicit', 'drill', '演练服玩家镜像快照的运行时间', 'schedule'],
  ['mirror-english', 'drill', 'When is the daily player snapshot scheduled?', 'schedule'],
  ['other-project', 'archive', '玩家镜像快照每天几点运行', 'otherSchedule'],
  ['model-explicit', 'drill', '记忆搜索用哪种数据库全文索引', 'model'],
  ['incident-cause', 'drill', '为什么结算后积分会重复', 'cause'],
  ['incident-remedy', 'drill', '上次对账用哪个字段做去重', 'remedy'],
  ['unfinished-explicit', 'drill', '配置文件改了怎么复核来源', 'unfinished'],
  ['report-in-project', 'archive', '排查报告先写什么', 'report'],
  ['report-in-fresh-chat', 'chat', '排查报告的结论和证据怎么排', 'report'],
  ['index-schedule', 'drill', '快照索引什么时候重建', 'reindex'],
  ['retention', 'drill', '快照清理会保留几天', 'retention'],
  ['file-format', 'drill', '快照导出的文件格式', 'export'],
  ['time-zone', 'drill', '快照日志用哪个时区', 'logZone'],
  ['same-chat', 'chat', '沙盘棋盘的边长是多少', 'boardSize', 'board'],
  ['different-chat', 'chat', '沙盘棋盘的边长是多少'],
  ['different-project', 'archive', 'round_key 去重'],
  ['unrelated', 'drill', '柠檬蛋糕需要烤多久'],
].map(([key, workspace, text, answer, sessionId = 'new-session']) => ({
  key,
  workspace,
  text,
  answer,
  sessionId,
}))

// 同一项目有很像的记忆，却没有问题所问的具体值。单独看它们是否占用注入预算。
export const unanswered = [
  ['retry-count', '玩家镜像快照失败后最多重试几次'],
  ['file-compression', '演练服快照导出文件的压缩等级是多少'],
  ['report-reviewer', '排查报告的审核人叫什么名字'],
]

// 成对问题仅替换一个已知意图的拼写，不访问记忆来猜正确单词；测的是纠正成功时的收益与代价。
// 特意保留“拼对后仍无对应词”、可见空间没有该事实、同一个术语下缺少答案等对照。
export const spellingPairs = [
  ['embedding-original', 'drill', 'embing', 'embedding', ' 模型用的什么', 'model'],
  ['export-format', 'drill', 'Parqeut', 'Parquet', ' 是这个项目的导出格式吗', 'export'],
  ['dedup-key', 'drill', 'round_kye', 'round_key', ' 用来解决哪种重复', 'remedy'],
  ['index-engine', 'drill', 'FTS55', 'FTS5', ' 在这里做什么', 'model'],
  ['log-zone', 'drill', 'timzeone', 'timezone', ' for snapshot logs', 'logZone'],
  ['compression-level', 'drill', 'Parqeut', 'Parquet', ' 的压缩等级是多少'],
  ['key-size', 'drill', 'round_kye', 'round_key', ' 的长度上限是多少'],
  ['index-version', 'drill', 'FTS55', 'FTS5', ' 的补丁版本号是多少'],
  ['wrong-workspace', 'archive', 'round_kye', 'round_key', ' 去重用的是哪一列'],
]

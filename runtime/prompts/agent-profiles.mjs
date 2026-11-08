// 内置技能 pisper-subagents 的正文：用户让代理新建或修改子代理定义时，模型据此写出能通过校验的文件。
// 限制由 runtime/services/agent-profiles.mjs 传入，规则一改技能文字跟着变；
// 不设 disable-model-invocation，模型按需自动加载。

/**
 * @param {{ maxProfiles: number, maxFileKb: number, maxDescription: number, maxInstructions: number, maxTools: number }} limits
 */
export function agentProfilesSkillMd({
  maxProfiles,
  maxFileKb,
  maxDescription,
  maxInstructions,
  maxTools,
}) {
  return `---
name: pisper-subagents
description: Read before creating or editing Pisper agent definitions (.pisper/agents/*.md — model tier, thinking level, tools, standing instructions).
---

# Pisper agent definitions

A definition is a Markdown file: frontmatter sets name, when to use it, model, thinking and tools; the body is standing instructions. When the parent calls \`spawn_agent\` with \`agent: <name>\`, the subagent uses that model, narrows its tools and appends the body to its system prompt.

## Location

- Project: \`<project root>/.pisper/agents/<name>.md\`, committed with the repo, applies to that project only. Default.
- Global: \`agents/<name>.md\` in the data directory, for all projects. This skill lives in \`<data dir>/builtin-skills/pisper-subagents/\`, so the global folder is \`agents/\` two levels up. Only when the user asks for all projects.
- A project definition overrides a global one with the same name. Max ${maxProfiles} definitions per location, ${maxFileKb} KB per file; only \`.md\` files directly in the folder are read.

## Format

~~~markdown
---
name: scout
description: Read-only code search; answers "where is X and how is it used". Does not edit files.
model: fast
thinking: low
tools: [read, grep, find, ls]
---

Report file paths and line numbers with the shortest relevant snippet. If nothing is found, say so; don't guess.
~~~

| Field | Required | Rules |
| --- | --- | --- |
| \`name\` | no | lowercase letters, digits, \`-\`, \`_\`; starts with a letter or digit; max 48 chars; defaults to the file name. |
| \`description\` | yes | max ${maxDescription} chars. The parent decides from this line alone: say what to delegate and what it won't do (e.g. "read-only"). |
| \`model\` | no | tier \`fast\` / \`standard\` / \`strong\`, or \`provider/modelId\`. Default: the parent's model. |
| \`tier\` | no | Only with \`provider/modelId\`: fallback tier when that model is unavailable. |
| \`thinking\` | no | one of \`off\`, \`minimal\`, \`low\`, \`medium\`, \`high\`, \`xhigh\`. Default: same as the parent. |
| \`tools\` | no | tool names (YAML list or comma-separated), max ${maxTools}. Must not be empty; omit it to keep all tools. |

The body is optional, max ${maxInstructions} chars. The parent writes each task separately, so the body holds only rules and output format that apply to every task.

## Models

- Prefer tiers over fixed models: the user maps tiers to models in Settings › Models › Subagents, so project files work across machines and providers. \`fast\`: lookups, summaries, formatting; \`standard\`: ordinary implementation and debugging; \`strong\`: design, hard problems, review.
- Write \`provider/modelId\` only when the user names a model, with a \`tier\` fallback.
- To pick a model by difficulty alone no definition is needed: pass \`tier\` to \`spawn_agent\`. A definition's model or tier wins.
- Fallback order: model → \`tier\` → tier passed to \`spawn_agent\` → parent model. The reason shows in the agent activity row and the \`spawn_agent\` result.
- An unmapped tier runs on the parent model; after writing, remind the user to map the tiers used.

## Tools

- \`tools\` only narrows: the subagent gets its intersection with the parent's tools. Tools reached via \`discover_tools\` (web search, MCP, ...) may be listed directly. Use exact names of tools you can see.
- Execution modes don't remove tools; calls are confirmed per mode as in the parent. Listed tools the parent lacks are skipped and reported in the \`spawn_agent\` result.
- Without \`tools\` the subagent gets the parent's directly available tools. \`get_plan\` and team tools are always kept.
- For read-only agents, leave out \`write\`, \`edit\`, \`bash\` and say "read-only" in \`description\`.
- Approval mode and workspace scope always follow the parent.

## After writing

- Definitions are re-read on every spawn; no restart needed.
- This session's \`spawn_agent\` description was built when the session opened and won't list a new definition; pass \`agent: <name>\` directly here. New sessions list it.
- A broken file returns the reason when spawned by name; global errors also show in Settings › Models › Subagents.
- Subagents cannot spawn subagents.
- Tell the user the file location, when it will be used, and which tier to map in settings.
`
}

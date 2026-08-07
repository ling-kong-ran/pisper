# Prompt and Tool Schema Sources

Pisper's system prompt and tool schemas follow the provider documentation below. The
runtime prompt keeps only stable operational rules; this file records the references
and the implementation decisions so provider guidance is reviewable without adding
reference URLs to every request prefix.

## Sources

- OpenAI, Prompt engineering: <https://platform.openai.com/docs/guides/prompt-engineering>
- OpenAI, Prompt caching: <https://platform.openai.com/docs/guides/prompt-caching>
- OpenAI, Function calling: <https://platform.openai.com/docs/guides/function-calling>
- Anthropic, Prompt engineering overview: <https://docs.anthropic.com/en/docs/build-with-claude/prompt-engineering/overview>
- Anthropic, Prompt caching: <https://docs.anthropic.com/en/docs/build-with-claude/prompt-caching>
- Anthropic, Tool use: <https://docs.anthropic.com/en/docs/agents-and-tools/tool-use/overview>
- Google, Gemini function calling: <https://ai.google.dev/gemini-api/docs/function-calling>
- JSON Schema, validation specification: <https://json-schema.org/draft/2020-12/json-schema-core>

## Applied Rules

- Keep stable instructions and tool definitions before request-specific context. Dynamic
  messages, requested optional tools, and other turn state stay in the per-turn context.
- Treat tool definitions as executable contracts. Names, required properties, types, and
  supported fields are validated server-side before authorization and execution.
- Keep optional capability discovery separate from execution. `discover_tools` returns a
  compact capability summary; `call_tool` routes the exact name and arguments through the
  real tool permission check.
- Keep descriptions short and action-oriented. Schema descriptions state what a field
  means and its constraints; they do not duplicate the whole system prompt.
- A tool result is data, not authorization. Workspace boundaries, execution mode, and
  approval policy remain enforced for the underlying tool.

The source links are provider documentation and standards references. Provider behavior
can vary by model and deployment, so `runtime/runtime/prompt-cache-diagnostics.mjs`
records changes to the stable prefix and request runtime configuration separately.

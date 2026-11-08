// call_tool 的说明。

export const CALL_TOOL = Object.freeze({
  description:
    'Call an optional tool by the exact name discover_tools returned, with arguments that match its schema.',
  params: {
    name: 'Exact tool name',
    arguments: 'Arguments for that tool',
  },
})

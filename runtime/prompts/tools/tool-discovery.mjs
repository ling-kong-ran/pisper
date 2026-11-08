// discover_tools 的说明。

export const DISCOVER_TOOLS = Object.freeze({
  description:
    'Find optional tools by capability: image/video generation & editing (generate_visual), web search, browser automation, mobile device control, memory, MCP, plugins and more.',
  guidelines: [
    'When the user wants generated images or video, device actions, web info, or other app capabilities, find the tool with discover_tools and call it instead of claiming it is unavailable.',
  ],
  params: {
    query: 'Capability or task',
    limit: 'Max matches (default 3)',
  },
})

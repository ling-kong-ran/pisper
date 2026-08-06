import assert from 'node:assert/strict'
import test from 'node:test'
import { Type } from 'typebox'
import { createToolGatewayTool, TOOL_GATEWAY_NAME } from '../tools/app/tool-gateway.mjs'
import { defineTool } from '../runtime/pi-coding-agent.mjs'

test('call_tool validates and executes an optional tool through its real identity', async () => {
  const calls = []
  const tool = defineTool({
    name: 'fixture.echo',
    description: 'Echo text',
    parameters: Type.Object({ text: Type.String() }),
    async execute(id, args) {
      calls.push({ id, args })
      return { content: [{ type: 'text', text: args.text }], details: {} }
    },
  })
  const gateway = createToolGatewayTool({
    getTool: (name) => (name === tool.name ? tool : null),
    authorize: async (input) => {
      assert.equal(input.toolName, 'fixture.echo')
      assert.deepEqual(input.args, { text: 'hello' })
      return undefined
    },
  })
  assert.equal(gateway.name, TOOL_GATEWAY_NAME)
  const result = await gateway.execute('gateway-1', {
    name: 'fixture.echo',
    arguments: { text: 'hello' },
  })
  assert.equal(result.content[0].text, 'hello')
  assert.equal(result.details.gatewayToolName, 'fixture.echo')
  assert.deepEqual(calls, [{ id: 'gateway-1:fixture.echo', args: { text: 'hello' } }])
})

test('call_tool rejects invalid arguments before authorization', async () => {
  let authorized = false
  const gateway = createToolGatewayTool({
    getTool: () =>
      defineTool({
        name: 'fixture.echo',
        parameters: Type.Object({ text: Type.String() }),
        async execute() {
          return { content: [{ type: 'text', text: 'never' }] }
        },
      }),
    authorize: async () => {
      authorized = true
    },
  })
  await assert.rejects(
    gateway.execute('gateway-2', { name: 'fixture.echo', arguments: { text: 42 } }),
    /Invalid arguments for fixture.echo/,
  )
  assert.equal(authorized, false)
})

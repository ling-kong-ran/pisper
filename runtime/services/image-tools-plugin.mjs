/** @typedef {'workflow' | 'workbench' | 'agent'} ImageToolsConsumer */

/** 内置素材插件只拥有能力准入，不拥有工作流、项目或会话状态。 */
export class ImageToolsPlugin {
  /** @param {{ isAgentEnabled?: () => boolean | Promise<boolean> }} [dependencies] */
  constructor({ isAgentEnabled = () => false } = {}) {
    this.isAgentEnabled = isAgentEnabled
  }

  /** @param {ImageToolsConsumer} consumer */
  async assertAllowed(consumer) {
    if (!['workflow', 'workbench', 'agent'].includes(consumer))
      throw Object.assign(new Error('image_tools_invalid_consumer'), {
        code: 'image_tools_invalid_consumer',
        statusCode: 400,
      })
    if (consumer === 'agent' && (await this.isAgentEnabled()) !== true)
      throw Object.assign(new Error('image_tools_agent_disabled'), {
        code: 'image_tools_agent_disabled',
        statusCode: 403,
      })
  }

  /**
   * consumer 由装配层固定绑定，不能由请求指定；缓存的 Agent 工具每次仍需检查开关。
   * @template Request, Result
   * @param {{execute:(request:Request, options?:{signal?:AbortSignal})=>Promise<Result>}} operations
   * @param {ImageToolsConsumer} consumer
   */
  bind(operations, consumer) {
    return {
      /** @param {Request} request @param {{signal?:AbortSignal}} [options] */
      execute: async (request, options = {}) => {
        options.signal?.throwIfAborted()
        await this.assertAllowed(consumer)
        options.signal?.throwIfAborted()
        return operations.execute(request, options)
      },
    }
  }
}

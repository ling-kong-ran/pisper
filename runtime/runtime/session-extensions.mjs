export function extensionToolNames(resourceLoader) {
  return [
    ...new Set(
      resourceLoader.getExtensions().extensions.flatMap((extension) => [...extension.tools.keys()]),
    ),
  ]
}

export async function bindSessionExtensions({ session, sessionId, cwd, extensions }) {
  await session.bindExtensions({
    mode: 'rpc',
    onError: (error) => extensions.recordRuntimeError(sessionId, cwd, error),
  })
  extensions.recordRuntime(sessionId, cwd, session.extensionRunner)
}

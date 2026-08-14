import { readFile } from 'node:fs/promises'
import { join } from 'node:path'

const MAX_PACKAGE_JSON_BYTES = 1024 * 1024

export async function execute({ toolName, context }) {
  if (toolName !== 'project_package_info') {
    throw new Error(`Unsupported tool: ${toolName}`)
  }

  const filePath = join(context.cwd, 'package.json')
  const source = await readFile(filePath, 'utf8')
  if (Buffer.byteLength(source, 'utf8') > MAX_PACKAGE_JSON_BYTES) {
    throw new Error('package.json exceeds the 1 MB example plugin limit.')
  }

  const packageJson = JSON.parse(source)
  const details = {
    name: String(packageJson.name || ''),
    version: String(packageJson.version || ''),
    description: String(packageJson.description || ''),
    packageManager: String(packageJson.packageManager || ''),
    scripts: Object.keys(packageJson.scripts || {}).sort(),
    filePath,
  }

  return {
    content: [
      {
        type: 'text',
        text: JSON.stringify(details, null, 2),
      },
    ],
    details,
  }
}

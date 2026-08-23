import { execFile } from 'node:child_process'
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { promisify } from 'node:util'

const run = promisify(execFile)

export function decodeTauriSignerPayload(value, label = 'Tauri signer payload') {
  const encoded = String(value || '').trim()
  if (
    !encoded ||
    encoded.length % 4 !== 0 ||
    !/^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/.test(encoded)
  ) {
    throw new Error(`${label} is not canonical Base64.`)
  }
  const decoded = Buffer.from(encoded, 'base64')
  if (decoded.toString('base64') !== encoded) {
    throw new Error(`${label} is not canonical Base64.`)
  }
  return decoded
}

export async function verifyTauriSignature({
  artifactPath,
  signaturePath = `${artifactPath}.minisig`,
  publicKeyPath = 'src-tauri/updater.pubkey',
  minisignCommand = 'minisign',
  minisignArgs = [],
}) {
  if (!artifactPath) throw new Error('Artifact path is required.')

  const temp = await mkdtemp(join(tmpdir(), 'pisper-tauri-signature-'))
  const decodedSignaturePath = join(temp, 'signature.minisig')
  const decodedPublicKeyPath = join(temp, 'public.key')
  try {
    const [encodedSignature, encodedPublicKey] = await Promise.all([
      readFile(resolve(signaturePath), 'utf8'),
      readFile(resolve(publicKeyPath), 'utf8'),
    ])
    await Promise.all([
      writeFile(
        decodedSignaturePath,
        decodeTauriSignerPayload(encodedSignature, 'Tauri signature'),
      ),
      writeFile(
        decodedPublicKeyPath,
        decodeTauriSignerPayload(encodedPublicKey, 'Tauri public key'),
      ),
    ])

    const { stdout, stderr } = await run(
      minisignCommand,
      [
        ...minisignArgs,
        '-Vm',
        resolve(artifactPath),
        '-x',
        decodedSignaturePath,
        '-p',
        decodedPublicKeyPath,
      ],
      { maxBuffer: 10 * 1024 * 1024 },
    )
    if (stdout) process.stdout.write(stdout)
    if (stderr) process.stderr.write(stderr)
  } finally {
    await rm(temp, { recursive: true, force: true })
  }
}

async function main() {
  const [artifactPath, signaturePath, publicKeyPath] = process.argv.slice(2)
  if (!artifactPath) {
    throw new Error(
      'Usage: node scripts/verify-tauri-signature.mjs <artifact> [signature] [public-key]',
    )
  }
  await verifyTauriSignature({ artifactPath, signaturePath, publicKeyPath })
}

const entryPath = process.argv[1] ? resolve(process.argv[1]) : ''
if (entryPath === fileURLToPath(import.meta.url)) {
  main().catch((error) => {
    console.error(error instanceof Error ? error.message : error)
    process.exitCode = 1
  })
}

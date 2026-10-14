import assert from 'node:assert/strict'
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import test from 'node:test'
import {
  decodeTauriSignerPayload,
  verifyTauriSignature,
} from '../../scripts/verify-tauri-signature.mjs'

test('Tauri signer payload decoder accepts canonical Base64 only', () => {
  const payload = 'untrusted comment: test\npayload\n'
  assert.equal(
    decodeTauriSignerPayload(Buffer.from(payload).toString('base64')).toString(),
    payload,
  )

  for (const invalid of ['', 'not-base64', 'YQ', 'YWJj===']) {
    assert.throws(() => decodeTauriSignerPayload(invalid), /canonical Base64/)
  }
})

test('Tauri signature verifier decodes updater files before invoking minisign', async () => {
  const root = await mkdtemp(join(tmpdir(), 'pisper-tauri-signature-test-'))
  const artifactPath = join(root, 'artifact.tar.gz')
  const signaturePath = `${artifactPath}.minisig`
  const publicKeyPath = join(root, 'updater.pubkey')
  const fakeMinisignPath = join(root, 'fake-minisign.mjs')
  const capturePath = join(root, 'capture.json')
  const signature =
    'untrusted comment: signature from tauri secret key\nsignature\ntrusted comment: test\nglobal\n'
  const publicKey = 'untrusted comment: minisign public key\npublic-key\n'

  try {
    await Promise.all([
      writeFile(artifactPath, 'artifact'),
      writeFile(signaturePath, Buffer.from(signature).toString('base64')),
      writeFile(publicKeyPath, Buffer.from(publicKey).toString('base64')),
      writeFile(
        fakeMinisignPath,
        `import fs from 'node:fs'\nconst args = process.argv.slice(2)\nconst value = (flag) => args[args.indexOf(flag) + 1]\nfs.writeFileSync(${JSON.stringify(
          capturePath,
        )}, JSON.stringify({ args, signature: fs.readFileSync(value('-x'), 'utf8'), publicKey: fs.readFileSync(value('-p'), 'utf8') }))\n`,
      ),
    ])

    await verifyTauriSignature({
      artifactPath,
      signaturePath,
      publicKeyPath,
      minisignCommand: process.execPath,
      minisignArgs: [fakeMinisignPath],
    })

    const capture = JSON.parse(await readFile(capturePath, 'utf8'))
    assert.equal(capture.args[0], '-Vm')
    assert.equal(capture.args[1], resolve(artifactPath))
    assert.equal(capture.signature, signature)
    assert.equal(capture.publicKey, publicKey)
  } finally {
    await rm(root, { recursive: true, force: true })
  }
})

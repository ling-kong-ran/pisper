import process from 'node:process'
import { ensurePisperInstallation } from './install.mjs'

if (process.env.PISPER_CLI_SKIP_INSTALL !== '1') {
  try {
    const installation = await ensurePisperInstallation()
    console.log(`pisper: installed the signed Pisper terminal client at ${installation.executable}`)
  } catch (error) {
    console.warn(
      `pisper: deferred terminal client download until the first pisper command: ${
        error instanceof Error ? error.message : String(error)
      }`,
    )
  }
}

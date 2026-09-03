declare abstract class AudioWorkletProcessor {
  readonly port: MessagePort
}

declare const sampleRate: number
declare function registerProcessor(
  name: string,
  processor: new (options?: AudioWorkletNodeOptions) => AudioWorkletProcessor,
): void

class PisperVoiceInputProcessor extends AudioWorkletProcessor {
  private readonly targetSampleRate: number
  private readonly inputSampleRate: number
  private readonly ratio: number
  private inputBuffer: number[] = []
  private readPosition = 0
  private outputBuffer: number[] = []

  constructor(options?: AudioWorkletNodeOptions) {
    super()
    this.targetSampleRate = Number(options?.processorOptions?.targetSampleRate) || 16_000
    this.inputSampleRate = sampleRate
    this.ratio = this.inputSampleRate / this.targetSampleRate
  }

  process(inputs: Float32Array[][], outputs: Float32Array[][]) {
    const channel = inputs[0]?.[0]
    if (channel?.length) {
      for (const sample of channel) this.inputBuffer.push(sample)

      while (this.readPosition + 1 < this.inputBuffer.length) {
        const index = Math.floor(this.readPosition)
        const fraction = this.readPosition - index
        const current = this.inputBuffer[index]
        const next = this.inputBuffer[index + 1]
        this.outputBuffer.push(current + (next - current) * fraction)
        this.readPosition += this.ratio
      }

      const consumed = Math.floor(this.readPosition)
      if (consumed > 0) {
        this.inputBuffer = this.inputBuffer.slice(consumed)
        this.readPosition -= consumed
      }

      while (this.outputBuffer.length >= 320) {
        const chunk = new Float32Array(this.outputBuffer.splice(0, 320))
        this.port.postMessage(chunk.buffer, [chunk.buffer])
      }
    }

    const output = outputs[0]?.[0]
    if (output) output.fill(0)
    return true
  }
}

registerProcessor('pisper-voice-input', PisperVoiceInputProcessor)

// AudioWorkletProcessor for voice capture.
// Replaces the deprecated ScriptProcessorNode (M6).
//
// Runs on the audio rendering thread. Collects PCM samples and posts
// them to the main thread when a full buffer is ready.
//
// Buffer size: 960 samples at 48 kHz = 20 ms frames.
// This matches the Opus codec standard frame size and provides much lower
// latency than the previous 4096-sample buffer (~85 ms -> ~20 ms).

class CaptureProcessor extends AudioWorkletProcessor {
  constructor() {
    super();
    // 960 samples at 48 kHz = 20 ms (standard Opus frame duration).
    // This is the optimal trade-off between latency and processing overhead.
    this._bufferSize = 960;
    this._buffer = new Float32Array(this._bufferSize);
    this._writeIndex = 0;
  }

  process(inputs, _outputs, _parameters) {
    const input = inputs[0];
    if (!input || input.length === 0) return true;

    const channelData = input[0]; // mono
    if (!channelData) return true;

    for (let i = 0; i < channelData.length; i++) {
      this._buffer[this._writeIndex++] = channelData[i];

      if (this._writeIndex >= this._bufferSize) {
        // Buffer full -- send a copy to the main thread
        this.port.postMessage({ pcmData: this._buffer.slice() });
        this._writeIndex = 0;
      }
    }

    return true; // keep processor alive
  }
}

registerProcessor("capture-processor", CaptureProcessor);

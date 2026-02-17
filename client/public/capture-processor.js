// AudioWorkletProcessor for voice capture.
// Replaces the deprecated ScriptProcessorNode (M6).
//
// Runs on the audio rendering thread. Collects PCM samples and posts
// them to the main thread when a full buffer is ready.

class CaptureProcessor extends AudioWorkletProcessor {
  constructor() {
    super();
    // Target ~4096 samples at native sample rate to match the old
    // ScriptProcessorNode buffer size.
    this._bufferSize = 4096;
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

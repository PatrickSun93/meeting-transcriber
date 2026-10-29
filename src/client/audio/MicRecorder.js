// Captures the local microphone and feeds 16 kHz mono audio to a SpeechChunker.
// ScriptProcessorNode is deprecated but works in every WebView Teams uses,
// including WKWebView on macOS, without loading a separate worklet file.
import SpeechChunker from './SpeechChunker.js';
import { downsample, rms } from './dsp.js';

const BUFFER_SIZE = 4096;

export default class MicRecorder {
  constructor({ onChunk, onLevel }) {
    this.onChunk = onChunk;
    this.onLevel = onLevel || (() => {});
    this.stream = null;
    this.context = null;
    this.source = null;
    this.processor = null;
    this.chunker = null;
  }

  async start() {
    if (!navigator.mediaDevices || !navigator.mediaDevices.getUserMedia) {
      throw new Error('Microphone access is not available here. The page must be served over HTTPS.');
    }
    this.stream = await navigator.mediaDevices.getUserMedia({
      audio: {
        channelCount: 1,
        echoCancellation: true,
        noiseSuppression: true,
        autoGainControl: true
      }
    });

    const AudioContextClass = window.AudioContext || window.webkitAudioContext;
    this.context = new AudioContextClass();
    if (this.context.state === 'suspended') {
      await this.context.resume();
    }
    const inputRate = this.context.sampleRate;

    this.chunker = new SpeechChunker({ onChunk: this.onChunk });
    this.source = this.context.createMediaStreamSource(this.stream);
    this.processor = this.context.createScriptProcessor(BUFFER_SIZE, 1, 1);
    this.processor.onaudioprocess = (event) => {
      const input = event.inputBuffer.getChannelData(0);
      this.onLevel(rms(input));
      this.chunker.push(downsample(input, inputRate));
    };
    this.source.connect(this.processor);
    // The processor only runs while connected to the destination; it outputs silence.
    this.processor.connect(this.context.destination);
  }

  async stop() {
    if (this.processor) {
      this.processor.onaudioprocess = null;
      this.processor.disconnect();
    }
    if (this.source) {
      this.source.disconnect();
    }
    if (this.stream) {
      this.stream.getTracks().forEach((track) => track.stop());
    }
    if (this.context) {
      await this.context.close();
    }
    if (this.chunker) {
      this.chunker.flush();
    }
    this.stream = this.context = this.source = this.processor = this.chunker = null;
    this.onLevel(0);
  }
}

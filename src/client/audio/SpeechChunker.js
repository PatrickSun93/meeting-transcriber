// Splits a continuous 16 kHz stream into utterances for Whisper.
// A chunk is cut at a pause once it is long enough, or forced at maxSeconds.
// Chunks with no frame above the speech threshold are dropped, since Whisper
// invents text for silence.
import { TARGET_SAMPLE_RATE, rms, concat } from './dsp.js';

const FRAME_SECONDS = 0.1;

export default class SpeechChunker {
  constructor({
    onChunk,
    sampleRate = TARGET_SAMPLE_RATE,
    minSeconds = 4,
    maxSeconds = 15,
    pauseSeconds = 0.7,
    speechThreshold = 0.01
  }) {
    this.onChunk = onChunk;
    this.sampleRate = sampleRate;
    this.frameSize = Math.round(sampleRate * FRAME_SECONDS);
    this.minSamples = minSeconds * sampleRate;
    this.maxSamples = maxSeconds * sampleRate;
    this.pauseFrames = Math.ceil(pauseSeconds / FRAME_SECONDS);
    this.speechThreshold = speechThreshold;
    this._reset();
    this._pending = new Float32Array(0);
    this._elapsedSamples = 0;
  }

  _reset() {
    this._frames = [];
    this._length = 0;
    this._silentFrames = 0;
    this._hasSpeech = false;
    this._startSample = null;
  }

  push(samples) {
    let data = this._pending.length ? concat([this._pending, samples]) : samples;
    let offset = 0;
    while (data.length - offset >= this.frameSize) {
      this._addFrame(data.subarray(offset, offset + this.frameSize));
      offset += this.frameSize;
    }
    this._pending = data.slice(offset);
  }

  _addFrame(frame) {
    if (this._startSample === null) {
      this._startSample = this._elapsedSamples;
    }
    this._elapsedSamples += frame.length;
    this._frames.push(Float32Array.from(frame));
    this._length += frame.length;

    const isSpeech = rms(frame) >= this.speechThreshold;
    if (isSpeech) {
      this._hasSpeech = true;
      this._silentFrames = 0;
    } else {
      this._silentFrames += 1;
    }

    // Long stretches of silence before any speech are just discarded.
    if (!this._hasSpeech && this._silentFrames >= this.pauseFrames) {
      this._reset();
      return;
    }

    const pausedAfterSpeech = this._silentFrames >= this.pauseFrames && this._length >= this.minSamples;
    if (pausedAfterSpeech || this._length >= this.maxSamples) {
      this._emit();
    }
  }

  _emit() {
    if (this._hasSpeech && this._length > 0) {
      this.onChunk({
        samples: concat(this._frames),
        startSeconds: this._startSample / this.sampleRate
      });
    }
    this._reset();
  }

  // Emit whatever is buffered, e.g. when recording stops.
  flush() {
    if (this._pending.length) {
      if (this._startSample === null) {
        this._startSample = this._elapsedSamples;
      }
      this._frames.push(this._pending);
      this._length += this._pending.length;
      this._elapsedSamples += this._pending.length;
      if (rms(this._pending) >= this.speechThreshold) {
        this._hasSpeech = true;
      }
      this._pending = new Float32Array(0);
    }
    this._emit();
  }
}

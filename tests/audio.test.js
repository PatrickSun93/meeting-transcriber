import { downsample, rms, concat } from '../src/client/audio/dsp.js';
import SpeechChunker from '../src/client/audio/SpeechChunker.js';

const RATE = 16000;

function tone(seconds, amplitude = 0.2) {
  const out = new Float32Array(Math.round(seconds * RATE));
  for (let i = 0; i < out.length; i++) {
    out[i] = amplitude * Math.sin((2 * Math.PI * 220 * i) / RATE);
  }
  return out;
}

const silence = (seconds) => new Float32Array(Math.round(seconds * RATE));

describe('dsp', () => {
  test('downsample 48 kHz to 16 kHz averages groups of three', () => {
    const out = downsample(Float32Array.from([1, 2, 3, 4, 5, 6]), 48000, 16000);
    expect(Array.from(out)).toEqual([2, 5]);
  });

  test('downsample 44.1 kHz gives the right length', () => {
    expect(downsample(new Float32Array(44100), 44100).length).toBe(16000);
  });

  test('downsample refuses to upsample', () => {
    expect(() => downsample(new Float32Array(10), 8000)).toThrow(/upsample/);
  });

  test('rms and concat', () => {
    expect(rms(Float32Array.from([3, -3, 3, -3]))).toBe(3);
    expect(rms(new Float32Array(0))).toBe(0);
    expect(Array.from(concat([Float32Array.from([1]), Float32Array.from([2, 3])]))).toEqual([1, 2, 3]);
  });
});

describe('SpeechChunker', () => {
  function collect(options = {}) {
    const chunks = [];
    const chunker = new SpeechChunker({ onChunk: (c) => chunks.push(c), ...options });
    return { chunker, chunks };
  }

  test('emits one chunk per utterance, cut at the pause', () => {
    const { chunker, chunks } = collect();
    chunker.push(tone(4.5));
    chunker.push(silence(1));
    chunker.push(tone(4.5));
    chunker.push(silence(1));
    expect(chunks).toHaveLength(2);
    // Cut after 0.7 s of pause; the rest of the pause starts the next chunk.
    expect(chunks[0].samples.length / RATE).toBeCloseTo(5.2, 1);
    expect(chunks[1].startSeconds).toBeCloseTo(5.2, 1);
  });

  test('drops pure silence', () => {
    const { chunker, chunks } = collect();
    chunker.push(silence(20));
    chunker.flush();
    expect(chunks).toHaveLength(0);
  });

  test('leading silence is trimmed and timestamps stay absolute', () => {
    const { chunker, chunks } = collect();
    chunker.push(silence(5));
    chunker.push(tone(4.5));
    chunker.push(silence(1));
    expect(chunks).toHaveLength(1);
    expect(chunks[0].startSeconds).toBeGreaterThanOrEqual(4.4);
    expect(chunks[0].startSeconds).toBeLessThanOrEqual(5);
  });

  test('forces a cut at maxSeconds during continuous speech', () => {
    const { chunker, chunks } = collect({ maxSeconds: 5 });
    chunker.push(tone(12));
    expect(chunks).toHaveLength(2);
    expect(chunks[0].samples.length).toBe(5 * RATE);
  });

  test('a short utterance waits for more audio, flush emits it', () => {
    const { chunker, chunks } = collect();
    chunker.push(tone(1));
    chunker.push(silence(0.7));
    expect(chunks).toHaveLength(0);
    chunker.flush();
    expect(chunks).toHaveLength(1);
  });

  test('handles input in odd-sized pieces', () => {
    const { chunker, chunks } = collect();
    const audio = concat([tone(4.5), silence(1)]);
    for (let i = 0; i < audio.length; i += 1365) {
      chunker.push(audio.subarray(i, i + 1365));
    }
    expect(chunks).toHaveLength(1);
  });
});

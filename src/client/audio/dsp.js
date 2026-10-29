// Small audio helpers. Whisper expects mono float32 samples at 16 kHz.
export const TARGET_SAMPLE_RATE = 16000;

// Downsample by averaging the input samples that fall into each output sample.
export function downsample(input, inputRate, outputRate = TARGET_SAMPLE_RATE) {
  if (inputRate === outputRate) {
    return Float32Array.from(input);
  }
  if (inputRate < outputRate) {
    throw new Error(`Cannot upsample from ${inputRate} Hz to ${outputRate} Hz`);
  }
  const ratio = inputRate / outputRate;
  const outputLength = Math.floor(input.length / ratio);
  const output = new Float32Array(outputLength);
  for (let i = 0; i < outputLength; i++) {
    const start = Math.floor(i * ratio);
    const end = Math.min(Math.floor((i + 1) * ratio), input.length);
    let sum = 0;
    for (let j = start; j < end; j++) {
      sum += input[j];
    }
    output[i] = sum / Math.max(end - start, 1);
  }
  return output;
}

export function rms(samples) {
  if (samples.length === 0) {
    return 0;
  }
  let sum = 0;
  for (let i = 0; i < samples.length; i++) {
    sum += samples[i] * samples[i];
  }
  return Math.sqrt(sum / samples.length);
}

export function concat(chunks) {
  const total = chunks.reduce((n, c) => n + c.length, 0);
  const out = new Float32Array(total);
  let offset = 0;
  for (const c of chunks) {
    out.set(c, offset);
    offset += c.length;
  }
  return out;
}

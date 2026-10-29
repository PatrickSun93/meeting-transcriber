// Local Whisper speech-to-text using transformers.js (ONNX runtime on the CPU).
// The model is downloaded once from Hugging Face and cached in ./models.
const path = require('path');

const SAMPLE_RATE = 16000;
const DEFAULT_MODEL = 'Xenova/whisper-base.en';

// Whisper tends to "hear" these on silence or noise; drop them.
const NOISE_PATTERNS = [
  /^\s*[[(].*[\])]\s*$/, // [BLANK_AUDIO], (music), (bells ringing)
  /^\s*(thank you|thanks for watching|you)[.!]?\s*$/i
];

function cleanText(text) {
  const trimmed = (text || '').trim();
  if (!trimmed || NOISE_PATTERNS.some((re) => re.test(trimmed))) {
    return '';
  }
  return trimmed;
}

function createWhisper({
  model = process.env.WHISPER_MODEL || DEFAULT_MODEL,
  cacheDir = path.resolve(__dirname, '../../models'),
  loadPipeline
} = {}) {
  let pipelinePromise = null;
  let ready = false;
  let loadError = null;

  async function defaultLoadPipeline() {
    const { pipeline, env } = await import('@xenova/transformers');
    env.cacheDir = cacheDir;
    env.allowLocalModels = false;
    return pipeline('automatic-speech-recognition', model, { quantized: true });
  }

  function load() {
    if (!pipelinePromise) {
      const started = Date.now();
      pipelinePromise = (loadPipeline || defaultLoadPipeline)(model)
        .then((asr) => {
          ready = true;
          loadError = null;
          console.log(`Whisper model ${model} ready in ${Date.now() - started} ms`);
          return asr;
        })
        .catch((err) => {
          loadError = err;
          pipelinePromise = null;
          throw err;
        });
    }
    return pipelinePromise;
  }

  // Audio calls are serialised: one CPU-bound transcription at a time.
  let queue = Promise.resolve();

  function transcribe(samples) {
    if (!(samples instanceof Float32Array)) {
      return Promise.reject(new TypeError('samples must be a Float32Array'));
    }
    const job = queue.then(async () => {
      const asr = await load();
      const started = Date.now();
      const result = await asr(samples, {
        chunk_length_s: 30,
        stride_length_s: 5
      });
      return {
        text: cleanText(result && result.text),
        durationMs: Date.now() - started,
        audioSeconds: samples.length / SAMPLE_RATE
      };
    });
    queue = job.catch(() => {});
    return job;
  }

  return {
    model,
    load,
    transcribe,
    status: () => ({
      model,
      ready,
      error: loadError ? String(loadError.message || loadError) : null
    })
  };
}

module.exports = { createWhisper, cleanText, SAMPLE_RATE };

// Express app: serves the Teams tab and the local transcription API.
const express = require('express');
const path = require('path');
const fs = require('fs');

const BUILD_DIR = path.resolve(__dirname, '../client/build');
// 16 kHz mono float32 is 64 KB per second, so 10 MB is about 2.5 minutes.
const MAX_AUDIO_BYTES = 10 * 1024 * 1024;

function createApp({ whisper }) {
  const app = express();

  app.get('/api/health', (req, res) => {
    res.json({ status: 'ok', whisper: whisper.status() });
  });

  // Body: raw little-endian float32 PCM samples, mono, 16 kHz.
  app.post(
    '/api/transcribe',
    express.raw({ type: 'application/octet-stream', limit: MAX_AUDIO_BYTES }),
    async (req, res) => {
      const body = req.body;
      if (!Buffer.isBuffer(body) || body.length === 0 || body.length % 4 !== 0) {
        return res.status(400).json({ error: 'Expected float32 PCM audio as application/octet-stream' });
      }
      // Copy into an aligned buffer; Node may hand us a slice at an odd offset.
      const samples = new Float32Array(body.length / 4);
      new Uint8Array(samples.buffer).set(body);
      try {
        const result = await whisper.transcribe(samples);
        res.json(result);
      } catch (err) {
        console.error('Transcription failed:', err);
        res.status(500).json({ error: 'Transcription failed', detail: String(err.message || err) });
      }
    }
  );

  app.use(express.static(BUILD_DIR, { index: false }));

  // Every page (/, /tab, /config) is the same single-page app.
  app.get(['/', '/tab', '/config'], (req, res) => {
    const indexFile = path.join(BUILD_DIR, 'index.html');
    if (!fs.existsSync(indexFile)) {
      return res.status(503).send('Client not built yet. Run "npm run build" first.');
    }
    res.sendFile(indexFile);
  });

  return app;
}

module.exports = { createApp };

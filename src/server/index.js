// Starts the server. Teams only loads tabs over HTTPS, so this serves HTTPS
// when certificates exist in ./certs (see "npm run certs"), else plain HTTP.
require('dotenv').config();
const fs = require('fs');
const http = require('http');
const https = require('https');
const path = require('path');
const { createApp } = require('./app');
const { createWhisper } = require('./whisper');

const PORT = Number(process.env.PORT) || 3001;
const CERT_DIR = path.resolve(__dirname, '../../certs');
const KEY_FILE = path.join(CERT_DIR, 'localhost-key.pem');
const CERT_FILE = path.join(CERT_DIR, 'localhost.pem');

const whisper = createWhisper();
const app = createApp({ whisper });

const useHttps = fs.existsSync(KEY_FILE) && fs.existsSync(CERT_FILE);
const server = useHttps
  ? https.createServer({ key: fs.readFileSync(KEY_FILE), cert: fs.readFileSync(CERT_FILE) }, app)
  : http.createServer(app);

server.listen(PORT, () => {
  const url = `${useHttps ? 'https' : 'http'}://localhost:${PORT}`;
  console.log(`Meeting Transcription running at ${url}`);
  if (!useHttps) {
    console.log('No certificates in ./certs: serving HTTP. Teams needs HTTPS; run "npm run certs".');
  }
  console.log(`Loading Whisper model ${whisper.model} (first run downloads it)...`);
  whisper.load().catch((err) => console.error('Failed to load Whisper model:', err.message));
});

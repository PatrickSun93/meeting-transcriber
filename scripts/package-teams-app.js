#!/usr/bin/env node
// Builds dist/meeting-transcription.zip for "Upload a custom app" in Teams.
// Usage: node scripts/package-teams-app.js [--url https://host[:port]]
// Defaults to APP_URL from the environment, then https://localhost:3001.
require('dotenv').config();
const fs = require('fs');
const path = require('path');
const archiver = require('archiver');

const ROOT = path.resolve(__dirname, '..');
const APP_DIR = path.join(ROOT, 'teams-app');
const OUT_DIR = path.join(ROOT, 'dist');
const OUT_FILE = path.join(OUT_DIR, 'meeting-transcription.zip');

function parseUrl(argv) {
  const i = argv.indexOf('--url');
  const raw = (i >= 0 && argv[i + 1]) || process.env.APP_URL || 'https://localhost:3001';
  const url = new URL(raw);
  if (url.protocol !== 'https:') {
    throw new Error(`Teams requires an https:// URL, got ${raw}`);
  }
  return url;
}

function buildManifest(url) {
  const appUrl = url.origin;
  const template = fs.readFileSync(path.join(APP_DIR, 'manifest.template.json'), 'utf8');
  const manifest = JSON.parse(
    template.replace(/{{APP_URL}}/g, appUrl).replace(/{{APP_HOST}}/g, url.host)
  );
  return manifest;
}

function writeZip(manifest) {
  fs.mkdirSync(OUT_DIR, { recursive: true });
  return new Promise((resolve, reject) => {
    const output = fs.createWriteStream(OUT_FILE);
    const archive = archiver('zip', { zlib: { level: 9 } });
    output.on('close', resolve);
    archive.on('error', reject);
    archive.pipe(output);
    archive.append(JSON.stringify(manifest, null, 2), { name: 'manifest.json' });
    archive.file(path.join(APP_DIR, 'color.png'), { name: 'color.png' });
    archive.file(path.join(APP_DIR, 'outline.png'), { name: 'outline.png' });
    archive.finalize();
  });
}

async function main() {
  const url = parseUrl(process.argv.slice(2));
  const manifest = buildManifest(url);
  await writeZip(manifest);
  console.log(`Created ${path.relative(ROOT, OUT_FILE)} for ${url.origin}`);
  console.log('In Teams: Apps > Manage your apps > Upload an app > Upload a custom app.');
}

if (require.main === module) {
  main().catch((err) => {
    console.error(err.message);
    process.exit(1);
  });
}

module.exports = { buildManifest, parseUrl };

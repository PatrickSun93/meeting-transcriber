# Technology Stack

## Current scope (demo)
A Teams tab that transcribes the local user's microphone with a local Whisper
model. Mac first. No cloud services, no Azure app registration.

## Core Technologies
- **Frontend**: React 18 function components, bundled by Webpack 5 + Babel
- **Backend**: Express 4 on Node 18+, HTTPS via mkcert certificates in `certs/`
- **Speech-to-text**: @xenova/transformers (Whisper, ONNX runtime) running in the Node server
- **Teams**: @microsoft/teams-js v2 (app init, context, tab config page)
- **Testing**: Jest (node + jsdom), @testing-library/react, supertest

## Commands
```bash
npm run build     # bundle the client into src/client/build
npm start         # serve https://localhost:3001 and load the Whisper model
npm run certs     # create trusted localhost certificates with mkcert
npm run package   # build dist/meeting-transcription.zip (--url to override host)
npm test          # Jest
npm run lint      # ESLint
```

## Code Style
2-space indentation, single quotes, semicolons (ESLint + Prettier).

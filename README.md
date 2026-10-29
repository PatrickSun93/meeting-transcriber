# Teams Meeting Transcription (local Whisper)

A Microsoft Teams app that transcribes what you say in a meeting. It runs as a meeting side panel or as a personal tab. Speech is turned into text on your own Mac by an OpenAI Whisper model, so audio never goes to a cloud service.

```
Teams tab (your mic) --16 kHz audio chunks--> local server (https://localhost:3001) --> Whisper --> text
```

**What it does today:** it captures **your microphone only** and labels each line with your Teams name. Other participants are not transcribed. Teams does not let a tab app read the meeting's audio; that needs a bot built on the Graph media APIs.

## Setup on a Mac

You need:

- **Node.js 18 or newer:** `brew install node`
- **mkcert**, to make a certificate Teams will trust: `brew install mkcert`
- **A Teams account that may upload custom apps.** Work and school accounts need this allowed by the admin. The free Microsoft 365 Developer Program tenant allows it. The free personal Teams app does not.

### 1. Install

```bash
git clone https://github.com/PatrickSun93/teams-meeting-plugin.git
cd teams-meeting-plugin
npm install
cp .env.example .env
```

### 2. Create the HTTPS certificate (once)

Teams only loads apps over HTTPS. This adds a local certificate authority to your keychain and asks for your Mac password once:

```bash
npm run certs
```

### 3. Build and start the server

```bash
npm run build
npm start
```

The first start downloads the Whisper model (about 75 MB for `whisper-base.en`) into `./models`. Wait for `Whisper model ... ready`.

To check it outside Teams, open https://localhost:3001 in Safari or Chrome, press **Start**, and talk. Text appears after each pause.

The server prints about 100 `[W:onnxruntime ... Removing initializer ...]` lines when the model loads. They are harmless.

### 4. Build the Teams app package

```bash
npm run package
```

This creates `dist/meeting-transcription.zip`.

### 5. Upload it to Teams

In Teams: **Apps → Manage your apps → Upload an app → Upload a custom app**, then pick `dist/meeting-transcription.zip`.

### 6. Use it

- **Personal tab:** open the app from the Apps list and use the **Transcription** tab.
- **In a meeting:** in the meeting window, click **Apps (+)**, choose **Meeting Transcription**, and click **Save**. Open it from the toolbar as a side panel.

Press **Start**. The first time, macOS asks to let Teams use the microphone. If you missed that prompt, allow it in **System Settings → Privacy & Security → Microphone → Microsoft Teams**, then restart Teams.

Keep `npm start` running while you use the app. The tab talks to the server on your Mac.

## Configuration (`.env`)

| Variable | Default | Notes |
| --- | --- | --- |
| `PORT` | `3001` | If you change it, rebuild the package with the matching URL. |
| `WHISPER_MODEL` | `Xenova/whisper-base.en` | `whisper-tiny.en` is faster; `whisper-small.en` is more accurate but slower; `whisper-base` handles other languages. |
| `APP_URL` | `https://localhost:3001` | The URL written into the Teams package. You can also pass `npm run package -- --url https://...`. |

## Troubleshooting

| Symptom | Fix |
| --- | --- |
| The tab in Teams is blank or shows an error | Is `npm start` running? Does https://localhost:3001 open in Safari without a certificate warning? If not, run `npm run certs` again and restart the server. |
| "Upload a custom app" is missing | Your organisation blocks custom apps. Use a Microsoft 365 developer tenant, or ask your admin to allow it. |
| "Cannot reach the local server" | Start the server with `npm start`. |
| Start stays greyed out | The model is still downloading or loading. Watch the server log. |
| Microphone permission denied | See step 6. |
| Odd words like "Thank you." in silence | These are known Whisper hallucinations. The server already filters the common ones. |

## Development

```bash
npm test         # Jest tests
npm run lint     # ESLint
npm run build    # bundle the client into src/client/build
```

The code layout is described in `.kiro/steering/structure.md`. The `browser-extension/` and `desktop-app/` folders hold earlier Google Meet and desktop work. They are not part of this demo.

## License

MIT

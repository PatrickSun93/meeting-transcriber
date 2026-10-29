# Project Structure

```
src/
  server/
    index.js        # starts HTTP(S) server, preloads the Whisper model
    app.js          # Express app: /api/health, /api/transcribe, static client
    whisper.js      # local Whisper pipeline, serial queue, hallucination filter
  client/
    index.js, App.js        # entry; routes /config vs /tab, Teams init and theme
    teams.js                # Teams SDK wrapper (init with timeout, tab config)
    transcribeClient.js     # fetch wrappers for the server API
    audio/
      dsp.js                # downsample to 16 kHz, rms, concat
      SpeechChunker.js      # splits audio into utterances at pauses
      MicRecorder.js        # getUserMedia + ScriptProcessor -> SpeechChunker
    components/
      TranscriptionTab.js   # main UI
      ConfigPage.js         # configurable tab setup page
public/index.html   # HTML template
teams-app/          # manifest template and icons
scripts/            # make-certs.sh, package-teams-app.js
tests/              # Jest tests
browser-extension/, desktop-app/  # earlier multi-platform work, not part of the demo
```

## Conventions
- Pure audio logic lives in `src/client/audio` and is unit tested without a browser.
- The server takes its Whisper instance by injection (`createApp({ whisper })`) so tests use a fake.

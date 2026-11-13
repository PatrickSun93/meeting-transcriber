# Voice agent roadmap: taking phone calls

**Status: planned, not built.** The voice agent ([voice-agent.md](voice-agent.md))
talks through a microphone and headphones on one Mac. This page is the plan for
a phone version: someone calls a real number and the same agent answers. None
of it is implemented. It is written so the work can start without first
re-reading Twilio's docs, and so the choices can be talked through.

## The idea in one picture

```
caller's phone ─▶ Twilio ─▶ wss://<ngrok>/media ─▶ μ-law 8 kHz ─▶ decode, 8→16 kHz ─▶ same pipeline as now
                                   ▲                                                     │
                                   └── media / mark / clear ◀── μ-law 8 kHz ◀── say at 8 kHz
```

The conversation loop stays exactly as it is. It already takes a stream of
16 kHz samples as input (today the microphone, or a file with `--input`) and
sends its voice through a small interface, `player::Output` (`rate`, `play`,
`stop`). A phone call is one more input and one more `Output`.

## Steps

### 1. Answer the call: TwiML

When the number is called, Twilio asks what to do. The answer is TwiML that
opens a two-way audio stream to us:

```xml
<Response>
  <Connect>
    <Stream url="wss://YOUR-NGROK-DOMAIN/media" />
  </Connect>
</Response>
```

`<Connect><Stream>` is the two-way kind (we can send audio back). `<Start><Stream>`
only listens, so it is not this.

For a demo, the simplest is a **TwiML Bin** in the Twilio console: no web
server of ours answers the call, and only the WebSocket needs to be
reachable. A webhook (`POST /voice` returning the XML) comes later, if the
TwiML has to change per call.

### 2. The WebSocket: Media Streams messages

Twilio connects to `wss://…/media` and sends JSON text messages:

| Message | What it carries | What we do |
|---|---|---|
| `connected` | protocol, version | nothing |
| `start` | `streamSid`, `callSid`, `mediaFormat` (`audio/x-mulaw`, 8000 Hz, 1 channel), custom parameters | keep `streamSid`; start a conversation for this call |
| `media` | `media.payload`: base64 μ-law, about 20 ms each; `track`, `chunk`, `timestamp` | decode and feed into the chunker and barge-in, like microphone packets |
| `mark` | the name of a mark we sent | that audio has finished playing (see step 4) |
| `dtmf` | a key pressed | ignore for now |
| `stop` | the call ended | finish the turn, save the record, close |

What we send back:

| Message | Shape |
|---|---|
| audio | `{"event":"media","streamSid":"…","media":{"payload":"<base64 μ-law 8 kHz>"}}`. Raw samples only: no WAV header, or it plays as noise |
| a mark | `{"event":"mark","streamSid":"…","mark":{"name":"t3-s1"}}`, sent right after a sentence's audio |
| stop talking | `{"event":"clear","streamSid":"…"}`. Twilio drops everything buffered and sends back every mark it hadn't reached yet |

*How:* a blocking WebSocket library (`tungstenite`) fits the program's
thread-per-part design: one thread reads messages, one writes. No async
runtime is needed. The server listens on `127.0.0.1:PORT` (`agent --twilio
--port 8080`); ngrok forwards the public `wss://` address to it.

### 3. Sound in: μ-law to 16 kHz

- **μ-law decode** (G.711): one byte becomes one 16-bit sample, through a
  256-entry table. It is about 20 lines, plus a test against known values,
  with no library needed.
- **8 → 16 kHz**: `audio::dsp::Resampler::to(8000, 16000)` already exists
  (interpolation; no filter is needed going up).
- **Thresholds**: phone audio is quieter, band-limited and compressed. The
  speech and barge-in levels (`--barge-in-level`) need measuring on real
  calls. Expect different values from the headset's.

### 4. Sound out: the phone as an `Output`

- **Voice**: `say` writes 8 kHz directly (`--data-format=LEI16@8000`), so no
  resampling is needed. `Output::rate()` returns 8000 and the voice thread
  already asks for that rate.
- **μ-law encode**: the standard bias-and-segment encoder (bias 0x84, clip
  at 32635), about 20 lines, tested by round-tripping through the decoder.
- **play**: encode, cut into 20 ms frames (160 bytes), send them as `media`,
  then a `mark` named after the turn and the sentence.
- **First sound and "how much was heard"**: today the player knows the
  exact sample. Over the phone we only know what Twilio tells us. A mark
  comes back when a sentence has played, so `Finished` comes from marks.
  `Started` is when the first frame was sent, which is earlier than the
  caller hears it by the network and Twilio's buffer. For a finer cut-off,
  send a mark every 200 ms or so instead of one per sentence. The cut-off
  sentence is then known to the nearest 200 ms.
- **stop (barge-in)**: send `clear`. Twilio returns the remaining marks: the
  earliest one returned says where playback stopped. The history keeps
  only what was heard, as now.

### 5. Security: only Twilio may connect

Opening a port on the Mac to the internet (through ngrok) means anyone who
finds the address can talk to the model and book "appointments". So:

- **Check `X-Twilio-Signature`** on the WebSocket upgrade request (the
  header arrives lowercase, `x-twilio-signature`). It is
  Base64(HMAC-SHA1(auth token, the full URL Twilio connected to)). For a
  webhook, the sorted POST parameters are appended to the URL as name+value.
  A refused handshake gets HTTP 403 and is logged. One report says the
  WebSocket's signature is computed over the `wss://` URL, not `https://`:
  check this against Twilio's validator on the first real call.
- The **auth token** comes from `TWILIO_AUTH_TOKEN`, never from a file in
  the repository. Without it the server does not start, unless an explicit
  `--insecure-no-signature` flag is given for a local test.
- **Expose nothing else.** No endpoint that hands out keys or tokens: model
  keys stay on the Mac. The server answers only `/media` (and `/voice` if
  the webhook is added).
- Bind to `127.0.0.1`: only ngrok reaches it.
- **Records**: a phone call's record holds the caller's number
  (`start.customParameters` or the webhook's `From`). Store it masked, or not
  at all.

### 6. Several calls at once, and no double bookings

- **One conversation per call**: each `start` gets its own loop (history,
  floor, record). They share one Whisper engine (a job queue, as meetings
  already have) and one model server.
- **No double booking**: today the check and the booking happen under one
  lock (the clinic is behind a `Mutex`, and `book_appointment` checks and
  writes in one call), so two calls in one process can't take the same
  time. A test should prove it: many threads book the same slot, exactly
  one succeeds, the rest get "already taken". If bookings ever go into a
  database, it must be a **unique constraint on (day, time)** or one
  transaction, never "SELECT, then INSERT" (that pattern lets two calls
  take the same slot).
- **Load**: Whisper and a 12B model on one laptop GPU serve one call well.
  A second simultaneous call roughly doubles the waits. Two calls at most
  for a demo, and say so.

### 7. Business time zone

Today "today" is the Mac's local date, which is fine on the desk. For a phone
line, the clinic's time zone decides:

- A setting such as `--timezone America/Los_Angeles` (an IANA zone; this
  needs a time zone table such as `chrono-tz`). "Today", "tomorrow" and the
  date in the system prompt are computed in that zone, wherever the server
  runs.
- **Past times are refused**: a time earlier today is neither offered by
  `check_availability` nor bookable. This is a known gap in the current
  demo too: it would offer this morning's slots in the afternoon.
- **Tests** at the edges: a server in UTC with the clinic in Los Angeles at
  23:30 and at 00:30 local; "tomorrow" on a Friday (the next open day is
  Monday); a daylight-saving change.

### 8. Testing without a phone

- **A fake Twilio**: a test client that connects to the server, sends
  `connected` and `start`, then a recorded caller as μ-law `media` frames in
  real time, and checks what comes back: `media` frames with valid μ-law,
  `mark`s after sentences, a `clear` after a barge-in, a refused handshake
  without a valid signature.
- **Unit tests**: μ-law decode and encode (known values, round trip), the
  message parsing (recorded Twilio JSON), the signature (Twilio's published
  example), mark bookkeeping (which sentence was cut, from the marks
  returned after `clear`).
- Then **one real call**, measured the same way as now: the latency table
  gets a phone column.

### 9. Setup steps (for whoever runs it)

1. Twilio account; buy a voice-capable number (see accounts below).
2. `ngrok http 8080` (better: a reserved ngrok domain, so the address
   doesn't change each time).
3. A TwiML Bin with the `<Connect><Stream>` above and the ngrok domain;
   attach it to the number under "A call comes in".
4. `export TWILIO_AUTH_TOKEN=…` (Twilio console, Account Info).
5. `meeting-transcriber agent --twilio --port 8080`, and call the number.

## Effort

Estimates for one person who knows this code, to a demo that works
reliably. They are not commitments.

| Step | Work | Estimate |
|---|---|---|
| WebSocket server, messages, `--twilio --port` | tungstenite, two threads, JSON in and out | 0.5 day |
| μ-law, resampling, the phone `Output` with marks and clear | encoder and decoder, tests | 0.5 day |
| Signature check | HMAC-SHA1 (a small dependency), tests with Twilio's example, the handshake hook | 0.5 day |
| Fake Twilio client and integration test | recorded caller → frames → checks | 0.5–1 day |
| Time zone and past times | setting, tests at the edges | 0.5 day |
| Several calls, the double-booking test | one loop per call, shared Whisper | 0.5 day |
| First real calls: thresholds, latency, fixes | needs the accounts | 0.5–1 day |
| **Total** | | **about 3.5 to 5 days** |

## Risks

- **Twilio trial limits**: a trial account plays a notice at the start of
  each call and only connects verified numbers (check the current terms).
  For an open demo, the account needs upgrading, which takes a card.
- **Latency gets worse**: the phone network, Twilio's buffer and ngrok all
  add delay, on top of today's 2.5–4 s. Expect the agent to feel slower on
  the phone than on the desk, and measure before promising a number.
- **Phone audio is harder for Whisper**: 8 kHz, compressed, sometimes noisy.
  More "say that again", and the end-of-turn pause may need tuning.
- **Echo on speakerphones**: phones cancel most of it, but a caller on
  speaker may still interrupt the agent with its own voice. Barge-in levels
  need real calls to set.
- **The Mac is the server**: it has to stay awake and online, with Ollama
  running, and the ngrok address has to match the TwiML Bin.
- **Exposure**: without the signature check, the open port lets anyone
  drive the model. The check is not optional.
- **Cost**: the number is a monthly fee and calls are charged by the minute
  (see Twilio's pricing page). Small for a demo, but not zero.

## Accounts Patrick opens himself

None of these are created, filled in or stored by code or by an AI agent:

- **Twilio**: the account, a phone number, the TwiML Bin, and the **auth
  token** (kept in his environment, `TWILIO_AUTH_TOKEN`).
- **ngrok**: the account, its authtoken (`ngrok config add-authtoken`), and
  optionally a reserved domain.
- **A model API key**, only if a hosted model (`--api openai|anthropic`) is
  used instead of Ollama, to cut latency on the phone.

## Staged pipeline or speech-to-speech?

A common way to build this is to hand the whole call to one speech-to-speech
model (OpenAI's Realtime API, for example): Twilio's μ-law audio goes in and
μ-law audio comes out, with recognition, the answer and the voice all inside
one service and its own end-of-turn detection. This project keeps the parts
separate: chunker → Whisper → LLM → sentences → TTS.

**Why separate parts here:**

- **Every step can be measured and explained.** The latency table exists
  because each stage has a start and an end. With one model there is only
  "audio in, audio out".
- **It runs locally**, with no key and no per-minute bill, and any part can
  be swapped (a faster model, another voice, streaming ASR).
- **Control at the text layer**: the exact words go into the history (only
  what was heard), tool arguments are checked before anything is booked,
  and the transcript is a real record.
- It reuses what the meeting app already does well.

**When speech-to-speech is the better choice:**

- When latency matters most: one hop instead of four, and the model can
  start answering before a full stop. It feels much more like a person,
  often under a second.
- When the voice has to sound natural: intonation, laughter, changing
  pace mid-sentence.
- When no one wants to run GPUs: a hosted service scales per call.

**What a switch gives up:**

- The per-stage numbers, and the ability to fix one stage alone.
- Running offline, and a fixed cost.
- Some control: what was said is reconstructed from the model's own
  transcript; tool calls still come back as text, but the guardrails move
  inside someone else's model.
- Choice: a different voice, or a different language model for the
  reasoning, isn't possible separately.

A middle way keeps this pipeline but swaps in streaming pieces: streaming
ASR with turn detection, a fast hosted LLM, and streaming TTS. That keeps
the control and the numbers and gets most of the speed.

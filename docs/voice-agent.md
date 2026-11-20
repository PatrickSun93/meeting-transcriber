# Voice agent (experimental demo)

`meeting-transcriber agent` is a voice agent you can talk to. It answers the
phone for a made-up clinic, checks which appointment times are free, books
one, and stops talking as soon as you talk over it. Everything can run on one
Mac, with no account and no internet.

It is a demo built on top of the meeting transcriber: the app already had the
**listening** half (finding where a sentence ends, live captions, the final
text). The agent adds the **thinking** half (a language model with tools) and
the **speaking** half (a voice that can be cut off).

> The clinic, its opening hours and which times are taken are invented and
> kept in memory. Nothing is booked anywhere.

## Running it

```bash
ollama pull gemma4:12b            # once; any Ollama chat model works with --model
meeting-transcriber agent         # put headphones on first
```

You hear "Hi, thanks for calling Maple Street Clinic…". Talk; pause; it
answers. The terminal shows both sides, each tool call, and a timing line for
every turn. Type `q` and Enter to hang up. The call is saved as Markdown in
`Documents/Meeting Transcripts/Voice Agent/`, in the same format as a meeting,
with a latency table in its notes.

Useful options (`meeting-transcriber help` lists them all):

| Option | What it changes |
|---|---|
| `--model NAME` | The Ollama model (default `gemma4:12b`) |
| `--api openai --url URL --model NAME` | An OpenAI-compatible server instead (key in `OPENAI_API_KEY`, none needed for LM Studio or vLLM) |
| `--api anthropic` | The Claude API (key in `ANTHROPIC_API_KEY`) |
| `--tools json` | Tools described in the prompt and answered as JSON text, for models whose own tool calling is unreliable |
| `--voice NAME`, `--rate WPM` | The `say` voice (default Samantha; `say -v '?'` lists them) and speed |
| `--pause SECONDS` | Silence that ends your turn (default 0.6) |
| `--barge-in-level RMS`, `--barge-in-ms MS` | How loud and how long you must talk to interrupt (default 0.02 for 200 ms) |
| `--no-barge-in` | The microphone is ignored while the agent speaks: for speakers instead of headphones |
| `--input FILE --no-sound` | A recording plays the caller and the voice is only timed, not played: for testing without a microphone |

**Use headphones.** With speakers, the microphone hears the agent's own voice
and takes it for you talking: it interrupts itself. `--no-barge-in` works with
speakers, but then you can't interrupt it.

## How it works, layer by layer

```
microphone ─▶ chunker ─▶ Whisper ─▶ LLM, streamed ─▶ sentences ─▶ say ─▶ speakers
                 │                     ▲    │
                 │                     │    └─▶ tools (check_availability, book_appointment)
                 └─▶ barge-in ─────────┴────────── stop everything ──────────┘
```

One loop owns the conversation: the history, who has the floor, the record.
Each slow part runs on its own thread and reports back on a channel, with
the number of the turn it belongs to. When a turn is cancelled, whatever it
still sends is recognised by that number and dropped. That one rule is
what makes interrupting safe.

### 1. Hearing: the chunker (`src/chunker.rs`, from the meeting app)

The microphone is cut into 100 ms frames. A frame louder than a small
threshold is speech. When speech has been followed by **0.6 s of silence**,
the utterance is over and goes to Whisper as a *final*. While you are still
talking, the audio so far goes to Whisper every second as a *partial*, which
is what you see on the `… ` line as you speak.

*Why:* it is simple and it already worked for meetings. The cost is that the
agent can only start after those 0.6 s of silence. That is the main reason it
feels slower than a person. A smarter end-of-turn detector (a model that
hears that a sentence is finished) is the obvious next step.

### 2. Understanding: Whisper (`src/transcriber.rs`)

The same local Whisper model as the meeting app (large-v3-turbo, quantised,
on the Mac's GPU through Metal). Two things make it fast enough for a
conversation:

- **A window the length of the clip.** Whisper's encoder normally looks at
  30 seconds of audio every time, padding a short clip with silence. The
  agent tells it to look only at the clip plus a second (`audio_ctx`). For a
  3-second sentence that took Whisper from about **1.0 s to about 0.3 s**.
  Meetings keep the full window: this is a switch only the agent turns on.
- **Finals before partials.** If Whisper is busy, old partials are thrown
  away; a final is never skipped.

A final with almost no real speech in it (Silero VAD, also from the meeting
app) is ignored, so a cough doesn't start a turn.

*Honest limitation:* this is **not streaming ASR**. Each partial runs Whisper
again on the whole growing buffer, and the final is one more full pass. A
streaming recogniser (Deepgram, or a local streaming model) would have the
text ready the moment you stop, instead of about 300 ms later.

### 3. Thinking: the language model (`src/agent/llm.rs`)

By default Ollama on the same Mac with `gemma4:12b`, through `/api/chat`
with `"stream": true`. The answer comes back one small piece (token) at a
time, one JSON object per line. The same code also speaks the OpenAI
streaming format and Claude's, both server-sent events, and builds the
history and tool list in each one's shape.

- **Streaming** means the voice can start on the first sentence while the
  model is still writing the second.
- **Cancelling** a turn stops reading and closes the connection, and
  Ollama stops generating.
- **Warm-up:** at start-up the model is loaded and the system prompt is
  sent once, so the first real turn doesn't pay for loading (about 8 s) or
  for reading the ~600-token prompt (about 4 s on this Mac). Measured: only
  sending the system prompt alone made Ollama reuse that work for the next
  request (0.5 s instead of 4 s).
- `"think": false`: no hidden reasoning first, since that would cost seconds
  of silence.

### 4. Tools (`src/agent/clinic.rs`)

Two tools: `check_availability(date, part_of_day)` and
`book_appointment(date, time, patient_name, reason)`, over invented data. The
system prompt gives today's date and the bookable days, so the model can turn
"Monday" into `2026-11-16` itself.

- **Native tool calling** is the default: gemma4 on Ollama returns a
  structured `tool_calls` list, which worked reliably in testing.
- **Every argument is checked** (a real date, an open day, a real slot, not
  already taken, a name). A bad call gets an error back that says what is
  wrong ("10:15 is not an appointment time…"), and the model corrects itself
  on the next pass.
- **JSON fallback** (`--tools json`, and always as a safety net): a reply
  that starts with `{` is held back from the voice, read as
  `{"tool": …, "arguments": …}`, and if it can't be read the model is asked
  once to answer again.
- **Filler:** a tool call needs a second pass through the model. If the
  model said nothing before calling the tool, the agent says "Let me check."
  or "One moment while I book that." right away, as a receptionist would.
- The prompt forbids saying "you're booked" before `book_appointment` has
  answered. Before that rule, gemma did exactly that.

### 5. Sentences (`src/agent/sentence.rs`)

The streamed text is cut into sentences: at `.`, `!` or `?` followed by a
space, at a line break, and at Chinese full stops. "10.30", "9:00 a.m." and
"Dr. Lee" don't end a sentence. Markdown the model writes despite being told
not to (`**`, `#`, list dashes) is removed, so it isn't read out.

The **first** piece of an answer may also end at a comma once it is 30
characters long, so the voice starts sooner on a long first sentence
(measured 3.4 s → 2.4 s to first sound in one test).

### 6. Speaking: `say` and our own player (`src/agent/tts.rs`, `player.rs`)

Each sentence becomes one `say` process that writes a WAV file at the
speakers' sample rate. The agent plays the file itself, through the same
audio library the app records with (cpal).

*Why not just let `say` play it?* Playing it ourselves tells us:

- **exactly when the sound starts** (the "first sound" number);
- **exactly how much of a sentence was heard** before an interruption, so
  only that goes into the history;
- and the next sentence is made **while this one plays**, so there are no
  gaps between sentences.

`say` costs about **0.35 to 0.6 s per sentence** to start (Samantha is one
of the slower voices; Daniel is faster). It is the simplest voice on a Mac.
A streaming TTS service would start speaking in about 100 ms.

The process is killed if the turn is cancelled while it runs.

### 7. Barge-in (`src/agent/floor.rs`)

The conversation is always in one of three states:

- **Listening**: the caller has the floor.
- **Thinking**: a turn is being worked on and the agent hasn't spoken yet.
- **Speaking**: the agent's voice is playing.

While the agent is **thinking or speaking**, the microphone is checked every
20 ms. **Ten loud frames in a row (200 ms) above a level** higher than the
chunker's means the caller is talking. The agent then:

1. stops the voice at once (the queue is emptied, the `say` process killed);
2. cancels the language model (the connection is closed);
3. writes into the history **only what was actually heard**: the sentences
   played to the end, plus the words of the cut-off sentence up to how much
   of its sound had played. The record shows it as
   `We are open Monday through Friday from nine AM to five PM, with a… [interrupted]`;
4. lets the caller's speech carry on into the chunker as the next turn.

Barge-in also works while it is still *thinking*. If you pause, the agent
starts answering, and you go on talking, that answer is dropped before you
hear it. A tool that already ran stays done (a booking is a booking), and
stays in the history.

*Why 200 ms and a higher level:* shorter, and a breath or a click
interrupts it. A real system would use a speech detector here, plus echo
cancellation so it works on a speakerphone.

### 8. The record (`src/agent/record.rs`)

Every turn is saved as it ends, so nothing is lost if the terminal is closed.
It is the meeting format (frontmatter, notes, `**[mm:ss] Caller:** …`), so
the app's meeting list can open it like any other.

## Latency: what the numbers mean, and what they are

Every turn prints, for example:

```
⏱ ASR 264 ms · LLM first token 2073 ms · first sentence to TTS 2222 ms · first sound 2993 ms
```

All are **milliseconds after the end of your speech was detected**, which is
itself 0.6 s after you actually stopped talking:

| Step | Means |
|---|---|
| ASR | Whisper has the final text |
| LLM first token | the first piece of the answer arrived (for a tool call: the whole call, since Ollama sends a call in one piece) |
| first sentence to TTS | the first sentence was handed to `say` |
| first sound | its first sample went to the speakers |

Measured on this Mac (Apple Silicon, large-v3-turbo-q5_0, gemma4:12b on
Ollama, voice Samantha), with recorded callers (`--input`), November 2026:

| Turn kind | ASR | LLM first token | First sentence | First sound |
|---|---|---|---|---|
| Plain answer | 260–410 ms | 0.9–1.3 s | 1.3–1.8 s | **1.9–2.4 s** |
| With a tool call | 260–360 ms | 2.1–3.1 s | 2.2–3.1 s | **2.8–3.7 s** |

So from the moment you stop talking, add the 0.6 s pause: **about 2.5 to 3 s**
for a plain answer and **3.4 to 4.3 s** with a tool. A person answers in well under a
second. Where the time goes, from biggest:

1. **The model** (gemma4 12B on a laptop GPU). A tool turn pays for it twice:
   once to decide on the call, once to answer with the result. A smaller
   model, a hosted fast model, or speculative "say the filler, start the
   tool" tricks all help.
2. **The 0.6 s end-of-turn pause.** A turn-detection model could cut it to
   0.2–0.3 s.
3. **`say` start-up, ~0.6 s.** A streaming TTS starts in ~0.1 s.
4. **Whisper, ~0.3 s.** Streaming ASR would bring it near zero.

Bluetooth headphones add another 150–250 ms after "first sound" that the
program can't see.

## What it can't do (yet)

- **macOS only.** The voice is `say`. On Windows and Linux the program
  builds and says clearly that there is no voice yet.
- **Headphones needed** for barge-in: there is no echo cancellation.
- **Not streaming ASR**, and end of turn is a fixed pause (see above).
- **English** by default (`--language`, `--voice` change it; the clinic
  prompt is English).
- **One scenario**, with invented data that is reset every run. Times
  earlier today are still offered and bookable.
- **No phone line.** A phone version (Twilio Media Streams) is planned but not
  built: [voice-agent-roadmap.md](voice-agent-roadmap.md) has the steps, the
  effort, the risks, and why this is a pipeline of separate parts rather than
  one speech-to-speech model.

## Testing it

- `cargo test agent` covers the pure parts without a microphone or a model:
  sentence cutting, the barge-in state machine, the three streaming formats
  (recorded streams, including gemma4's tool calls), tool-call checking and
  the JSON fallback, the player's queue and cut-off, the record.
- End to end without a microphone: make a caller with `say` and play it in.

  ```bash
  say -v Daniel -o caller.wav --file-format=WAVE --data-format=LEI16@16000 \
      "Hi, do you have anything open on Monday morning?"
  # add a few seconds of silence around it (any audio editor or sox), then
  meeting-transcriber agent --input caller.wav --no-sound
  ```

## Code

| File | Role |
|---|---|
| `src/agent/mod.rs` | The command, the conversation loop, the Whisper and voice threads, one turn's model-and-tools loop |
| `src/agent/llm.rs` | Messages, request bodies and stream parsers for Ollama, OpenAI and Claude; the streaming HTTP call |
| `src/agent/clinic.rs` | The scenario: prompt, greeting, demo data, the two tools, the JSON fallback |
| `src/agent/sentence.rs` | Streamed text into sentences |
| `src/agent/floor.rs` | Listening / thinking / speaking, and barge-in |
| `src/agent/tts.rs` | `say` |
| `src/agent/player.rs` | The voice's queue, the speakers, and a silent clock for tests |
| `src/agent/record.rs` | Latency marks and the Markdown record |

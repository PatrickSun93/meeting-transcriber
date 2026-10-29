import { useCallback, useEffect, useRef, useState } from 'react';
import MicRecorder from '../audio/MicRecorder.js';
import { createSerialQueue, getHealth, transcribeSamples } from '../transcribeClient.js';

const HEALTH_POLL_MS = 2000;

function formatClock(seconds) {
  const s = Math.max(0, Math.floor(seconds));
  const mm = String(Math.floor(s / 60)).padStart(2, '0');
  const ss = String(s % 60).padStart(2, '0');
  return `${mm}:${ss}`;
}

export function transcriptToText(lines) {
  return lines.map((l) => `[${formatClock(l.startSeconds)}] ${l.speaker}: ${l.text}`).join('\n');
}

export default function TranscriptionTab({ userName = 'You', inTeams = false }) {
  const [model, setModel] = useState({ ready: false, model: null, error: null });
  const [serverError, setServerError] = useState(null);
  const [recording, setRecording] = useState(false);
  const [pending, setPending] = useState(0);
  const [level, setLevel] = useState(0);
  const [lines, setLines] = useState([]);
  const [error, setError] = useState(null);
  const [copied, setCopied] = useState(false);

  const recorderRef = useRef(null);
  const queueRef = useRef(createSerialQueue());
  const sessionOffsetRef = useRef(0);
  const lastEndRef = useRef(0);
  const endRef = useRef(null);

  // Poll the server until the Whisper model has loaded.
  useEffect(() => {
    let cancelled = false;
    let timer = null;
    const poll = async () => {
      try {
        const health = await getHealth();
        if (cancelled) return;
        setServerError(null);
        setModel(health.whisper);
        if (health.whisper.ready) return;
      } catch (err) {
        if (cancelled) return;
        setServerError('Cannot reach the local server. Is "npm start" running?');
      }
      timer = setTimeout(poll, HEALTH_POLL_MS);
    };
    poll();
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, []);

  useEffect(() => {
    if (endRef.current && endRef.current.scrollIntoView) {
      endRef.current.scrollIntoView({ block: 'end' });
    }
  }, [lines]);

  const handleChunk = useCallback(
    ({ samples, startSeconds }) => {
      const absoluteStart = sessionOffsetRef.current + startSeconds;
      setPending((n) => n + 1);
      queueRef.current(() => transcribeSamples(samples))
        .then(({ text }) => {
          if (text) {
            setLines((prev) => [
              ...prev,
              { id: `${absoluteStart}-${prev.length}`, startSeconds: absoluteStart, speaker: userName, text }
            ]);
          }
        })
        .catch((err) => setError(`Transcription failed: ${err.message}`))
        .finally(() => setPending((n) => n - 1));
      lastEndRef.current = Math.max(lastEndRef.current, absoluteStart + samples.length / 16000);
    },
    [userName]
  );

  const start = async () => {
    setError(null);
    // Continue the clock after a previous stop instead of restarting at 00:00.
    sessionOffsetRef.current = lastEndRef.current;
    const recorder = new MicRecorder({ onChunk: handleChunk, onLevel: setLevel });
    try {
      await recorder.start();
      recorderRef.current = recorder;
      setRecording(true);
    } catch (err) {
      await recorder.stop().catch(() => {});
      const denied = err && (err.name === 'NotAllowedError' || err.name === 'SecurityError');
      setError(
        denied
          ? 'Microphone permission was denied. Allow microphone access for Teams in System Settings > Privacy & Security > Microphone, then try again.'
          : `Could not start the microphone: ${err.message}`
      );
    }
  };

  const stop = async () => {
    const recorder = recorderRef.current;
    recorderRef.current = null;
    setRecording(false);
    if (recorder) {
      await recorder.stop();
    }
  };

  useEffect(() => () => {
    if (recorderRef.current) {
      recorderRef.current.stop();
    }
  }, []);

  const copy = async () => {
    const text = transcriptToText(lines);
    try {
      await navigator.clipboard.writeText(text);
    } catch (err) {
      // Teams iframes often block the async clipboard API; use the old path.
      const area = document.createElement('textarea');
      area.value = text;
      document.body.appendChild(area);
      area.select();
      const ok = document.execCommand && document.execCommand('copy');
      document.body.removeChild(area);
      if (!ok) {
        setError('Copy failed. Use Download instead.');
        return;
      }
    }
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  };

  const download = () => {
    const blob = new Blob([transcriptToText(lines)], { type: 'text/plain' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = `transcript-${new Date().toISOString().slice(0, 16).replace(/[:T]/g, '-')}.txt`;
    a.click();
    URL.revokeObjectURL(url);
  };

  const canStart = model.ready && !serverError && !recording;
  let status;
  if (serverError) status = serverError;
  else if (model.error) status = `Whisper model failed to load: ${model.error}`;
  else if (!model.ready) status = 'Loading the Whisper model on your Mac (first run downloads it)...';
  else if (recording) status = pending > 0 ? 'Listening, transcribing...' : 'Listening...';
  else status = 'Ready';

  return (
    <div className="tab">
      <header className="tab-header">
        <div>
          <h1>Meeting Transcription</h1>
          <p className="muted">
            Transcribes your microphone locally with Whisper{model.model ? ` (${model.model})` : ''}.
            {inTeams ? '' : ' Running outside Teams.'}
          </p>
        </div>
        <div className="controls">
          {recording ? (
            <button className="btn btn-stop" onClick={stop}>Stop</button>
          ) : (
            <button className="btn btn-primary" onClick={start} disabled={!canStart}>Start</button>
          )}
        </div>
      </header>

      <div className="status-row" role="status">
        <span className={`dot ${recording ? 'dot-live' : model.ready ? 'dot-ready' : 'dot-wait'}`} />
        <span>{status}</span>
        {recording && (
          <span className="meter" aria-label="Microphone level">
            <span className="meter-fill" style={{ width: `${Math.min(100, level * 400)}%` }} />
          </span>
        )}
      </div>

      {error && (
        <div className="error" role="alert">
          {error}
          <button className="link" onClick={() => setError(null)}>Dismiss</button>
        </div>
      )}

      <main className="transcript" aria-live="polite">
        {lines.length === 0 ? (
          <p className="muted empty">
            {recording ? 'Start speaking. Text appears after each pause.' : 'Press Start and speak.'}
          </p>
        ) : (
          lines.map((line) => (
            <p key={line.id} className="line">
              <span className="time">{formatClock(line.startSeconds)}</span>
              <span className="speaker">{line.speaker}</span>
              <span className="text">{line.text}</span>
            </p>
          ))
        )}
        <div ref={endRef} />
      </main>

      <footer className="tab-footer">
        <button className="btn" onClick={copy} disabled={lines.length === 0}>{copied ? 'Copied' : 'Copy'}</button>
        <button className="btn" onClick={download} disabled={lines.length === 0}>Download .txt</button>
        <button className="btn" onClick={() => setLines([])} disabled={lines.length === 0 || recording}>Clear</button>
      </footer>
    </div>
  );
}

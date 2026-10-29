import { useEffect, useState } from 'react';
import { registerTabConfig } from '../teams.js';

export default function ConfigPage({ inTeams }) {
  const [error, setError] = useState(null);

  useEffect(() => {
    if (!inTeams) return;
    try {
      registerTabConfig();
    } catch (err) {
      setError(String(err.message || err));
    }
  }, [inTeams]);

  return (
    <div className="tab config">
      <h1>Meeting Transcription</h1>
      <p>Adds a Transcription panel to this meeting. Press <strong>Save</strong> to continue.</p>
      <p className="muted">
        Audio is transcribed on your Mac with a local Whisper model and is not sent anywhere else.
      </p>
      {!inTeams && <p className="muted">This page is only used inside Teams when adding the app to a meeting.</p>}
      {error && <div className="error" role="alert">{error}</div>}
    </div>
  );
}

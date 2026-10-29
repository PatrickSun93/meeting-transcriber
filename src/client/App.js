import { useEffect, useState } from 'react';
import { app } from '@microsoft/teams-js';
import TranscriptionTab from './components/TranscriptionTab.js';
import ConfigPage from './components/ConfigPage.js';
import { describeContext, initTeams } from './teams.js';
import './App.css';

function applyTheme(theme) {
  document.body.dataset.theme = theme === 'dark' || theme === 'contrast' ? theme : 'default';
}

export default function App({ path = window.location.pathname }) {
  const [teams, setTeams] = useState(null);

  useEffect(() => {
    let active = true;
    initTeams().then((result) => {
      if (!active) return;
      const info = describeContext(result.context);
      applyTheme(info.theme);
      if (result.inTeams) {
        app.registerOnThemeChangeHandler(applyTheme);
      }
      setTeams({ inTeams: result.inTeams, ...info });
    });
    return () => {
      active = false;
    };
  }, []);

  if (!teams) {
    return <div className="tab muted">Loading...</div>;
  }
  if (path.startsWith('/config')) {
    return <ConfigPage inTeams={teams.inTeams} />;
  }
  return <TranscriptionTab userName={teams.userName} inTeams={teams.inTeams} />;
}

// Teams SDK wrapper. The same page also runs in a normal browser, so the
// SDK init is raced against a timeout instead of awaited forever.
import { app, pages } from '@microsoft/teams-js';

const INIT_TIMEOUT_MS = 3000;

export async function initTeams() {
  try {
    await Promise.race([
      app.initialize(),
      new Promise((_, reject) => setTimeout(() => reject(new Error('timeout')), INIT_TIMEOUT_MS))
    ]);
    const context = await app.getContext();
    app.notifySuccess();
    return { inTeams: true, context };
  } catch (err) {
    return { inTeams: false, context: null };
  }
}

// Config page for the meeting side panel / chat tab: there is nothing to
// choose, so it is valid right away and saves the tab URL.
export function registerTabConfig() {
  const origin = window.location.origin;
  pages.config.registerOnSaveHandler((saveEvent) => {
    pages.config
      .setConfig({
        entityId: 'meeting-transcription',
        contentUrl: `${origin}/tab`,
        websiteUrl: `${origin}/tab`,
        suggestedDisplayName: 'Transcription'
      })
      .then(() => saveEvent.notifySuccess())
      .catch((err) => saveEvent.notifyFailure(String(err)));
  });
  pages.config.setValidityState(true);
}

export function describeContext(context) {
  if (!context) {
    return { userName: 'You', meetingId: null, frameContext: null, theme: 'default' };
  }
  const user = context.user || {};
  return {
    userName: user.displayName || user.userPrincipalName || 'You',
    meetingId: context.meeting ? context.meeting.id : null,
    frameContext: context.page ? context.page.frameContext : null,
    theme: context.app ? context.app.theme : 'default'
  };
}

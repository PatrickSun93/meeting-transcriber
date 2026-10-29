/**
 * @jest-environment jsdom
 */
import { render, screen, fireEvent, waitFor, act } from '@testing-library/react';
import '@testing-library/jest-dom';
import TranscriptionTab, { transcriptToText } from '../src/client/components/TranscriptionTab.js';

let mockOnChunk = null;
jest.mock('../src/client/audio/MicRecorder.js', () =>
  jest.fn().mockImplementation(({ onChunk }) => {
    mockOnChunk = onChunk;
    return { start: jest.fn().mockResolvedValue(), stop: jest.fn().mockResolvedValue() };
  })
);

function mockServer({ ready = true, text = 'Hello team.' } = {}) {
  global.fetch = jest.fn(async (url) => {
    if (url === '/api/health') {
      return { ok: true, json: async () => ({ status: 'ok', whisper: { ready, model: 'Xenova/whisper-base.en', error: null } }) };
    }
    return { ok: true, json: async () => ({ text }) };
  });
}

beforeAll(() => {
  Element.prototype.scrollIntoView = jest.fn();
});

test('start is disabled until the model is ready', async () => {
  mockServer({ ready: false });
  render(<TranscriptionTab userName="Patrick" />);
  expect(await screen.findByText(/Loading the Whisper model/)).toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Start' })).toBeDisabled();
});

test('recorded chunks are transcribed and shown with the speaker name', async () => {
  mockServer();
  render(<TranscriptionTab userName="Patrick" />);
  const startButton = await screen.findByRole('button', { name: 'Start' });
  await waitFor(() => expect(startButton).toBeEnabled());
  fireEvent.click(startButton);
  await screen.findByRole('button', { name: 'Stop' });

  await act(async () => {
    mockOnChunk({ samples: new Float32Array(16000), startSeconds: 65 });
  });
  expect(await screen.findByText('Hello team.')).toBeInTheDocument();
  expect(screen.getByText('Patrick')).toBeInTheDocument();
  expect(screen.getByText('01:05')).toBeInTheDocument();

  const call = global.fetch.mock.calls.find(([url]) => url === '/api/transcribe');
  expect(call[1].headers['Content-Type']).toBe('application/octet-stream');
  expect(call[1].body.byteLength).toBe(16000 * 4);
});

test('server errors are shown', async () => {
  global.fetch = jest.fn(async () => {
    throw new Error('connection refused');
  });
  render(<TranscriptionTab />);
  expect(await screen.findByText(/Cannot reach the local server/)).toBeInTheDocument();
});

test('transcriptToText formats lines', () => {
  expect(
    transcriptToText([
      { startSeconds: 3, speaker: 'A', text: 'Hi.' },
      { startSeconds: 125, speaker: 'A', text: 'Bye.' }
    ])
  ).toBe('[00:03] A: Hi.\n[02:05] A: Bye.');
});

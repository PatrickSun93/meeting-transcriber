const request = require('supertest');
const { createApp } = require('../src/server/app');
const { createWhisper, cleanText } = require('../src/server/whisper');

function fakeWhisper(text = ' Hello world. ') {
  const calls = [];
  const whisper = createWhisper({
    model: 'fake/model',
    loadPipeline: async () => async (samples) => {
      calls.push(samples);
      return { text };
    }
  });
  return { whisper, calls };
}

const toBody = (floats) => Buffer.from(Float32Array.from(floats).buffer);

describe('server', () => {
  test('health reports model status', async () => {
    const { whisper } = fakeWhisper();
    const app = createApp({ whisper });
    let res = await request(app).get('/api/health');
    expect(res.body.whisper).toEqual({ model: 'fake/model', ready: false, error: null });
    await whisper.load();
    res = await request(app).get('/api/health');
    expect(res.body.whisper.ready).toBe(true);
  });

  test('transcribe decodes float32 audio and returns cleaned text', async () => {
    const { whisper, calls } = fakeWhisper();
    const res = await request(createApp({ whisper }))
      .post('/api/transcribe')
      .set('Content-Type', 'application/octet-stream')
      .send(toBody([0.5, -0.25, 0.125]));
    expect(res.status).toBe(200);
    expect(res.body.text).toBe('Hello world.');
    expect(Array.from(calls[0])).toEqual([0.5, -0.25, 0.125]);
  });

  test('transcribe rejects bad bodies', async () => {
    const { whisper } = fakeWhisper();
    const app = createApp({ whisper });
    const empty = await request(app).post('/api/transcribe').set('Content-Type', 'application/octet-stream');
    expect(empty.status).toBe(400);
    const odd = await request(app)
      .post('/api/transcribe')
      .set('Content-Type', 'application/octet-stream')
      .send(Buffer.from([1, 2, 3]));
    expect(odd.status).toBe(400);
  });

  test('transcribe reports model errors as 500', async () => {
    const whisper = createWhisper({
      loadPipeline: async () => {
        throw new Error('download failed');
      }
    });
    const res = await request(createApp({ whisper }))
      .post('/api/transcribe')
      .set('Content-Type', 'application/octet-stream')
      .send(toBody([0]));
    expect(res.status).toBe(500);
    expect(res.body.detail).toMatch(/download failed/);
    expect(whisper.status().error).toMatch(/download failed/);
  });

  test('transcriptions run one at a time, in order', async () => {
    const order = [];
    const whisper = createWhisper({
      loadPipeline: async () => async (samples) => {
        const n = samples[0];
        order.push(`start ${n}`);
        await new Promise((r) => setTimeout(r, n === 1 ? 30 : 1));
        order.push(`end ${n}`);
        return { text: String(n) };
      }
    });
    const results = await Promise.all([1, 2, 3].map((n) => whisper.transcribe(Float32Array.from([n]))));
    expect(results.map((r) => r.text)).toEqual(['1', '2', '3']);
    expect(order).toEqual(['start 1', 'end 1', 'start 2', 'end 2', 'start 3', 'end 3']);
  });

  test('cleanText drops Whisper silence hallucinations', () => {
    expect(cleanText('[BLANK_AUDIO]')).toBe('');
    expect(cleanText(' (bells ringing) ')).toBe('');
    expect(cleanText('Thank you.')).toBe('');
    expect(cleanText(' We ship on Friday. ')).toBe('We ship on Friday.');
  });
});

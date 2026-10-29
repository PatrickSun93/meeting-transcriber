// Talks to the local server's Whisper endpoint.

export async function transcribeSamples(samples, fetchImpl = fetch) {
  const response = await fetchImpl('/api/transcribe', {
    method: 'POST',
    headers: { 'Content-Type': 'application/octet-stream' },
    body: samples.buffer.slice(samples.byteOffset, samples.byteOffset + samples.byteLength)
  });
  const data = await response.json().catch(() => ({}));
  if (!response.ok) {
    throw new Error(data.detail || data.error || `Server returned ${response.status}`);
  }
  return data;
}

export async function getHealth(fetchImpl = fetch) {
  const response = await fetchImpl('/api/health');
  if (!response.ok) {
    throw new Error(`Server returned ${response.status}`);
  }
  return response.json();
}

// Runs async jobs one at a time, in the order they were added, so transcript
// lines come back in the order they were spoken.
export function createSerialQueue() {
  let tail = Promise.resolve();
  return (job) => {
    const result = tail.then(job);
    tail = result.catch(() => {});
    return result;
  };
}

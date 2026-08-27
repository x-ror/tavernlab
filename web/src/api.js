/* One place that knows the server's HTTP shape.
 *
 * The server is `tavernsim serve` — the Rust simulator with an HTTP face on
 * it. Reads are GET, everything that takes a deck code is POST, and the two
 * runs long enough to want a progress line (rating a deck, computing the
 * tier matrix) answer with a job id that `pollJob` follows. */

async function parse(res, path) {
  let data = null
  try {
    data = await res.json()
  } catch {
    data = null
  }
  if (data === null) throw new Error(`HTTP ${res.status} ${path}`)
  // A body carrying `ok` is a structured answer, not a failure: /api/resolve
  // says *why* a deck will not load in fields the caller translates, and
  // throwing its English `error` away would lose them.
  if (data.error && !data.job && data.ok === undefined) throw new Error(data.error)
  return data
}

export async function post(path, body) {
  const res = await fetch(path, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body || {}),
  })
  return parse(res, path)
}

export async function get(path) {
  const res = await fetch(path, { headers: { Accept: 'application/json' } })
  return parse(res, path)
}

/** Poll a background job. `onProgress` gets the whole log array. */
export async function pollJob(jobId, onProgress, signal) {
  for (;;) {
    if (signal?.aborted) throw new Error('aborted')
    const res = await fetch(`/api/job/${jobId}`)
    const job = await res.json()
    if (job.error && !job.status) throw new Error(job.error)
    onProgress?.(job.progress || [])
    if (job.status !== 'running') {
      if (job.status === 'error' || job.error) throw new Error(job.error || 'job failed')
      return job.result
    }
    await new Promise((r) => setTimeout(r, 700))
  }
}

/** POST that starts a job, then waits for it. */
export async function runJob(path, body, onProgress, signal) {
  const started = await post(path, body)
  if (started.error) throw new Error(started.error)
  return pollJob(started.job, onProgress, signal)
}

export const settingsApi = {
  read: () => get('/api/settings'),
  write: (patch) => post('/api/settings', patch),
}

export const metrics = () => get('/api/metrics')
export const cardNames = (all = true) => post('/api/cardnames', { all })

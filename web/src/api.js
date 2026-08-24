/* One place that knows the Python app's HTTP shape.
 * `api()` is POST-only on the server side, GET routes need plain fetch —
 * that split is the server's (design §2.7), so it is hidden here. */

async function parse(res, path) {
  let data = null
  try { data = await res.json() } catch (e) { data = null }
  if (data === null) throw new Error(`HTTP ${res.status} ${path}`)
  if (data.error && !data.job) throw new Error(data.error)
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

export const games = {
  list: (q = {}) => {
    const p = new URLSearchParams()
    p.set('limit', String(q.limit || 100))
    if (q.cls) p.set('class', q.cls)
    if (q.result) p.set('result', q.result)
    return get('/api/games?' + p.toString())
  },
  one: (id) => get(`/api/games/${id}`),
  review: (id) => get(`/api/games/${id}/review`),
  replay: (id) => get(`/api/games/${id}/replay`),
  analyze: (id) => post(`/api/games/${id}/analyze`),
  reparse: (id) => post(`/api/games/${id}/reparse`),
}

export const settingsApi = {
  read: () => get('/api/settings'),
  write: (patch) => post('/api/settings', patch),
}

export const metrics = () => get('/api/metrics')
export const cardNames = (all = true) => post('/api/cardnames', { all })

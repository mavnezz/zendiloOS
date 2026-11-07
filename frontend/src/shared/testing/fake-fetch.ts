/**
 * A stand-in for the server in component tests: it answers the requests of
 * the API client (and of plain `fetch`) from handlers the test sets, and
 * records them. The API client takes `fetch` when it is loaded, so this
 * module imports nothing and is loaded first (through `./server`).
 */
/** A request as the server got it. */
export interface Call {
  method: string
  path: string
  query: URLSearchParams
  body: unknown
  headers: Headers
}

/** What a handler answers: a response, data sent as JSON, or nothing (204). */
type Handler = (call: Call) => unknown

const handlers: { method: string; path: string | RegExp; handler: Handler }[] = []

/** Every request since the last {@link resetServer}, oldest first. */
export const calls: Call[] = []

/** Answers `method path` (a path, or a pattern for it) with `handler`; later ones win. */
export function on(method: string, path: string | RegExp, handler: Handler): void {
  handlers.push({ method, path, handler })
}

/** Answers `method path` with `data`, always. */
export function answer(method: string, path: string | RegExp, data: unknown): void {
  on(method, path, () => data)
}

/** Problem details, as the server sends them. */
export function problem(status: number, code: string, detail?: string): Response {
  return new Response(JSON.stringify({ type: 'about:blank', title: '', status, code, detail }), {
    status,
    headers: { 'Content-Type': 'application/problem+json' },
  })
}

/** The requests made as `method path`. */
export function callsTo(method: string, path: string | RegExp): Call[] {
  return calls.filter((call) => call.method === method && matches(path, call.path))
}

export function resetServer(): void {
  handlers.length = 0
  calls.length = 0
}

function matches(path: string | RegExp, actual: string): boolean {
  return typeof path === 'string' ? path === actual : path.test(actual)
}

async function serve(input: RequestInfo | URL, init?: RequestInit): Promise<Response> {
  const request =
    input instanceof Request ? input : new Request(new URL(String(input), 'http://localhost'), init)
  const url = new URL(request.url)
  const text =
    request.method === 'GET' || request.method === 'HEAD' ? '' : await request.clone().text()
  let body: unknown = text || undefined
  try {
    body = text ? JSON.parse(text) : undefined
  } catch {
    // Not JSON: kept as text.
  }
  const call = {
    method: request.method,
    path: url.pathname,
    query: url.searchParams,
    body,
    headers: request.headers,
  }
  calls.push(call)
  const found = [...handlers]
    .reverse()
    .find((h) => h.method === call.method && matches(h.path, call.path))
  if (!found) return problem(404, 'not_found', `${call.method} ${call.path}`)
  const result = await found.handler(call)
  if (result instanceof Response) return result
  if (result === undefined) return new Response(null, { status: 204 })
  return new Response(JSON.stringify(result), { headers: { 'Content-Type': 'application/json' } })
}

globalThis.fetch = serve

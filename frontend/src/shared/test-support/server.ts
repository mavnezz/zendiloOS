/**
 * A stand-in for the API in component tests: answers by method and path,
 * and keeps every request. Tests route the client to it with
 *
 *     vi.mock('@/shared/api/client', async (original) =>
 *       (await import('@/shared/test-support/server')).routed(await original()))
 */

export interface Call {
  method: string
  path: string
  query: Record<string, string>
  body: unknown
}

/** What a route answers: a body, a `Response`, `null` for none, or a function of the request. */
type Reply = unknown

const routes = new Map<string, Reply>()
export const calls: Call[] = []

/** Answers `method path` (the path without query) with `reply` from now on. */
export function on(method: string, path: string, reply: Reply): void {
  routes.set(`${method} ${path}`, reply)
}

/** Forgets every route and request. */
export function reset(): void {
  routes.clear()
  calls.length = 0
}

/** The requests made to `method path`. */
export function called(method: string, path: string): Call[] {
  return calls.filter((call) => call.method === method && call.path === path)
}

export function problem(status: number, code: string, detail?: string): Response {
  return new Response(JSON.stringify({ type: 'about:blank', title: '', status, code, detail }), {
    status,
    headers: { 'Content-Type': 'application/problem+json' },
  })
}

async function body(request: Request): Promise<unknown> {
  const text = await request.text()
  if (!text) return undefined
  try {
    return JSON.parse(text) as unknown
  } catch {
    return text
  }
}

export async function answer(request: Request): Promise<Response> {
  const url = new URL(request.url)
  const call: Call = {
    method: request.method,
    path: decodeURIComponent(url.pathname),
    query: Object.fromEntries(url.searchParams),
    body: request.method === 'GET' ? undefined : await body(request),
  }
  calls.push(call)
  const reply = routes.get(`${call.method} ${call.path}`)
  if (reply === undefined) return problem(404, 'not_found')
  const result: unknown =
    typeof reply === 'function' ? await (reply as (call: Call) => unknown)(call) : reply
  // A copy: a body is read only once, and a route may be asked again.
  if (result instanceof Response) return result.clone()
  if (result === null) return new Response(null, { status: 204 })
  return new Response(JSON.stringify(result), { headers: { 'Content-Type': 'application/json' } })
}

/** The client module with its `api` answered here. */
export function routed<
  T extends { createApiClient: (options: { baseUrl?: string; fetch?: typeof fetch }) => unknown },
>(real: T): T {
  return {
    ...real,
    api: real.createApiClient({
      baseUrl: 'http://nas.test',
      fetch: (input) => answer(input as Request),
    }),
  }
}

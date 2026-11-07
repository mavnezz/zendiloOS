import { afterEach, describe, expect, it, vi } from 'vitest'

import { ATTENDED_MS, noteInput } from './activity'
import { ApiProblem, configureApi, createApiClient, unwrap } from './client'

function problem(status: number, code: string, headers: Record<string, string> = {}): Response {
  return new Response(JSON.stringify({ type: 'about:blank', title: '', status, code }), {
    status,
    headers: { 'Content-Type': 'application/problem+json', ...headers },
  })
}

function json(body: unknown): Response {
  return new Response(JSON.stringify(body), { headers: { 'Content-Type': 'application/json' } })
}

function clientAnswering(response: () => Response) {
  const requests: Request[] = []
  const client = createApiClient({
    baseUrl: 'http://nas.test',
    fetch: (input) => {
      requests.push(input as Request)
      return Promise.resolve(response())
    },
  })
  return { client, requests }
}

afterEach(() => configureApi({ csrfToken: () => undefined, onUnauthenticated: () => undefined }))

describe('the API client', () => {
  it('sends the CSRF token with state-changing requests only', async () => {
    configureApi({ csrfToken: () => 'csrf-1', onUnauthenticated: () => undefined })
    const { client, requests } = clientAnswering(() => new Response(null, { status: 204 }))

    await client.GET('/api/v1/auth/session')
    await client.POST('/api/v1/auth/logout')

    expect(requests[0]?.headers.get('X-CSRF-Token')).toBeNull()
    expect(requests[1]?.headers.get('X-CSRF-Token')).toBe('csrf-1')
  })

  it('marks reads nobody asked for just now, never what the person does', async () => {
    configureApi({ csrfToken: () => 'csrf-1', onUnauthenticated: () => undefined })
    const { client, requests } = clientAnswering(() => new Response(null, { status: 204 }))

    noteInput()
    await client.GET('/api/v1/auth/session')
    noteInput(Date.now() - ATTENDED_MS - 1000)
    await client.GET('/api/v1/auth/session')
    await client.POST('/api/v1/auth/logout')
    noteInput()

    expect(requests[0]?.headers.get('X-Zendiloos-Background')).toBeNull()
    expect(requests[1]?.headers.get('X-Zendiloos-Background')).toBe('1')
    expect(requests[2]?.headers.get('X-Zendiloos-Background')).toBeNull()
  })

  it('turns problem details into an ApiProblem', async () => {
    const { client } = clientAnswering(() =>
      problem(429, 'too_many_attempts', { 'Retry-After': '900' }),
    )

    const failure = await unwrap(
      client.POST('/api/v1/auth/login', { body: { name: 'a', password: 'b' } }),
    ).catch((error: unknown) => error)

    expect(failure).toBeInstanceOf(ApiProblem)
    expect(failure).toMatchObject({
      status: 429,
      code: 'too_many_attempts',
      retryAfterSeconds: 900,
    })
  })

  it('returns the data of successful answers', async () => {
    const { client } = clientAnswering(() => json({ required: true }))
    await expect(unwrap(client.GET('/api/v1/setup'))).resolves.toEqual({ required: true })
  })

  it('reports an unreachable server as a network problem', async () => {
    const client = createApiClient({
      baseUrl: 'http://nas.test',
      fetch: () => Promise.reject(new TypeError('Failed to fetch')),
    })
    await expect(unwrap(client.GET('/api/v1/setup'))).rejects.toMatchObject({ code: 'network' })
  })

  it('reports an ended session, but not a failed login', async () => {
    const onUnauthenticated = vi.fn()
    configureApi({ csrfToken: () => undefined, onUnauthenticated })
    const { client } = clientAnswering(() => problem(401, 'unauthenticated'))

    await client.POST('/api/v1/auth/login', { body: { name: 'a', password: 'b' } })
    await client.GET('/api/v1/auth/session')
    expect(onUnauthenticated).not.toHaveBeenCalled()

    await client.GET('/api/v1/system/info')
    expect(onUnauthenticated).toHaveBeenCalledOnce()
  })
})

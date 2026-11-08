import createClient, { type Middleware } from 'openapi-fetch'

import { unattended, watchActivity } from './activity'
import type { paths } from './schema'

/** An error answer of the API (RFC 9457), or `network` if there was none. */
export class ApiProblem extends Error {
  constructor(
    readonly status: number,
    /** Stable code from the API, e.g. `invalid_credentials`. */
    readonly code: string,
    readonly detail?: string,
    readonly retryAfterSeconds?: number,
  ) {
    super(detail ?? code)
    this.name = 'ApiProblem'
  }
}

export interface ApiHooks {
  /** CSRF token of the current session, sent with every state-changing request. */
  csrfToken: () => string | undefined
  /** The session ended (expired, revoked, logged out elsewhere). */
  onUnauthenticated: () => void
}

let hooks: ApiHooks = { csrfToken: () => undefined, onUnauthenticated: () => undefined }

/** Connects the client to the session; called once at startup. */
export function configureApi(next: ApiHooks): void {
  hooks = next
}

/** The CSRF token, for requests that cannot go through {@link api}, e.g. uploads with progress. */
export function csrfToken(): string | undefined {
  return hooks.csrfToken()
}

const SAFE_METHODS = new Set(['GET', 'HEAD', 'OPTIONS'])

/** 401 is the normal answer here and says nothing about an existing session. */
const UNAUTHENTICATED_IS_EXPECTED = [
  '/api/v1/auth/login',
  '/api/v1/auth/login/second-factor',
  '/api/v1/auth/session',
]

const sessionMiddleware: Middleware = {
  onRequest({ request }) {
    const token = hooks.csrfToken()
    if (token && !SAFE_METHODS.has(request.method)) {
      request.headers.set('X-CSRF-Token', token)
    }
    // A read nobody asked for just now keeps no session alive.
    if (SAFE_METHODS.has(request.method) && unattended()) {
      request.headers.set('X-Zendiloos-Background', '1')
    }
    return request
  },
  onResponse({ request, response }) {
    const path = new URL(request.url).pathname
    if (response.status === 401 && !UNAUTHENTICATED_IS_EXPECTED.includes(path)) {
      hooks.onUnauthenticated()
    }
    return response
  },
}

export function createApiClient(options: { baseUrl?: string; fetch?: typeof fetch } = {}) {
  if (typeof window !== 'undefined') watchActivity()
  const client = createClient<paths>({
    baseUrl: options.baseUrl ?? window.location.origin,
    fetch: options.fetch,
    credentials: 'same-origin',
  })
  client.use(sessionMiddleware)
  return client
}

export const api = createApiClient()

interface Result<T> {
  data?: T
  error?: unknown
  response: Response
}

/** The data of a successful answer; throws an {@link ApiProblem} otherwise. */
export async function unwrap<T>(request: Promise<Result<T>>): Promise<T> {
  let result: Result<T>
  try {
    result = await request
  } catch {
    throw new ApiProblem(0, 'network')
  }
  const { data, error, response } = result
  if (!response.ok) {
    const problem = (typeof error === 'object' && error !== null ? error : {}) as {
      code?: unknown
      detail?: unknown
    }
    const retryAfter = Number(response.headers.get('Retry-After'))
    throw new ApiProblem(
      response.status,
      typeof problem.code === 'string' ? problem.code : 'unknown',
      typeof problem.detail === 'string' ? problem.detail : undefined,
      Number.isFinite(retryAfter) && retryAfter > 0 ? retryAfter : undefined,
    )
  }
  return data as T
}

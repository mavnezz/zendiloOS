import { beforeEach, describe, expect, it } from 'vitest'

import { answer, on, problem, reset } from './server'

beforeEach(reset)

describe('the stand-in for the API', () => {
  it('answers a route asked again with the same response, not one already read', async () => {
    on('POST', '/api/v1/plan', problem(422, 'invalid_input', 'a bond needs at least one member'))
    const ask = () =>
      answer(new Request('http://nas.test/api/v1/plan', { method: 'POST', body: '{}' }))
    for (const response of [await ask(), await ask()]) {
      expect(response.status).toBe(422)
      expect(((await response.json()) as { detail: string }).detail).toBe(
        'a bond needs at least one member',
      )
    }
  })
})

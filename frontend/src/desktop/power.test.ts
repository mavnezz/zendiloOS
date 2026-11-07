import { createPinia, setActivePinia } from 'pinia'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { live } from '@/desktop/testing/live'
import { called, on, problem, reset } from '@/desktop/testing/server'

import { usePower } from './power'

vi.mock('@/shared/api/client', async (original) =>
  (await import('@/desktop/testing/server')).routed(await original()),
)
vi.mock('@/shared/api/live', async () => ({ live: (await import('@/desktop/testing/live')).live }))

const reload = vi.fn()
const original = window.location

beforeEach(() => {
  setActivePinia(createPinia())
  reset()
  reload.mockClear()
  live.connect()
  Object.defineProperty(window, 'location', { configurable: true, value: { ...original, reload } })
})

afterEach(() => {
  vi.useRealTimers()
  vi.unstubAllGlobals()
  Object.defineProperty(window, 'location', { configurable: true, value: original })
})

describe('restarting and shutting down', () => {
  it('shuts down: the connection goes, the page says so', async () => {
    on('POST', '/api/v1/system/power', null)
    const power = usePower()
    await power.request('power_off')
    expect(called('POST', '/api/v1/system/power')[0]?.body).toEqual({ action: 'power_off' })
    expect(power.phase).toBe('off')
    expect(live.connected).toBe(false)
  })

  it('reloads once the NAS was away and is back', async () => {
    vi.useFakeTimers()
    on('POST', '/api/v1/system/power', null)
    const answers = [false, false, true]
    vi.stubGlobal(
      'fetch',
      vi.fn(() => {
        const up = answers.shift() ?? true
        return up ? Promise.resolve(new Response('{}', { status: 200 })) : Promise.reject(new Error('down'))
      }),
    )
    const power = usePower()
    await power.request('reboot')
    expect(power.phase).toBe('restarting')
    for (let round = 0; round < 4 && !reload.mock.calls.length; round += 1) {
      await vi.advanceTimersByTimeAsync(3000)
    }
    expect(reload).toHaveBeenCalledTimes(1)
  })

  it('reloads anyway when the restart went by unnoticed', async () => {
    vi.useFakeTimers()
    on('POST', '/api/v1/system/power', null)
    vi.stubGlobal('fetch', vi.fn(() => Promise.resolve(new Response('{}', { status: 200 }))))
    await usePower().request('reboot')
    await vi.advanceTimersByTimeAsync(60_000)
    expect(reload).not.toHaveBeenCalled()
    await vi.advanceTimersByTimeAsync(40_000)
    expect(reload).toHaveBeenCalledTimes(1)
  })

  it('leaves everything as it was when the server refuses', async () => {
    on('POST', '/api/v1/system/power', problem(409, 'jobs_running', 'files.copy photos'))
    const power = usePower()
    await expect(power.request('reboot')).rejects.toMatchObject({ code: 'jobs_running' })
    expect(power.phase).toBe('running')
    expect(live.connected).toBe(true)
  })
})

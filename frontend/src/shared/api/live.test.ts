import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { LiveClient, type SocketLike } from './live'
import type { Metrics } from './types'

class FakeSocket implements SocketLike {
  readyState = 0
  sent: unknown[] = []
  onopen: ((event: Event) => void) | null = null
  onmessage: ((event: MessageEvent) => void) | null = null
  onclose: ((event: CloseEvent) => void) | null = null
  send(data: string) {
    this.sent.push(JSON.parse(data))
  }
  close() {
    this.readyState = 3
  }
  opened() {
    this.readyState = 1
    this.onopen?.(new Event('open'))
  }
  receive(message: unknown) {
    this.onmessage?.(new MessageEvent('message', { data: JSON.stringify(message) }))
  }
  dropped(code = 1006) {
    this.readyState = 3
    this.onclose?.(new CloseEvent('close', { code }))
  }
}

const metrics = { at: 1, cpu_percent: 5 } as Metrics

let sockets: FakeSocket[]
let client: LiveClient

beforeEach(() => {
  vi.useFakeTimers()
  sockets = []
  client = new LiveClient(
    () => 'ws://nas/api/v1/ws',
    () => {
      const socket = new FakeSocket()
      sockets.push(socket)
      return socket
    },
  )
})

afterEach(() => vi.useRealTimers())

describe('LiveClient', () => {
  it('subscribes once the socket is open and delivers messages', () => {
    const received: Metrics[] = []
    client.subscribe('metrics', (data) => received.push(data))
    client.connect()
    sockets[0]!.opened()
    sockets[0]!.receive({ type: 'metrics', data: metrics })

    expect(sockets[0]!.sent).toEqual([{ type: 'subscribe', topic: 'metrics' }])
    expect(received).toEqual([metrics])
  })

  it('unsubscribes when the last listener leaves', () => {
    client.connect()
    sockets[0]!.opened()
    const first = client.subscribe('metrics', () => undefined)
    const second = client.subscribe('metrics', () => undefined)
    first()
    second()

    expect(sockets[0]!.sent).toEqual([
      { type: 'subscribe', topic: 'metrics' },
      { type: 'unsubscribe', topic: 'metrics' },
    ])
  })

  it('reconnects with growing delays and subscribes again', () => {
    client.subscribe('metrics', () => undefined)
    client.connect()
    sockets[0]!.dropped()
    vi.advanceTimersByTime(999)
    expect(sockets).toHaveLength(1)
    vi.advanceTimersByTime(1)
    expect(sockets).toHaveLength(2)

    sockets[1]!.dropped()
    vi.advanceTimersByTime(1999)
    expect(sockets).toHaveLength(2)
    vi.advanceTimersByTime(1)
    sockets[2]!.opened()
    expect(sockets[2]!.sent).toEqual([{ type: 'subscribe', topic: 'metrics' }])
  })

  it('stops when the session has ended', () => {
    const ended = vi.fn()
    client.onSessionEnded = ended
    client.connect()
    sockets[0]!.dropped(4401)
    vi.advanceTimersByTime(60_000)

    expect(ended).toHaveBeenCalledOnce()
    expect(sockets).toHaveLength(1)
  })

  it('says when it is back after a drop, so lists are loaded again', () => {
    const back = vi.fn()
    const off = client.onReconnect(back)
    client.connect()
    sockets[0]!.opened()
    expect(back).not.toHaveBeenCalled()

    sockets[0]!.dropped()
    vi.advanceTimersByTime(1000)
    sockets[1]!.opened()
    expect(back).toHaveBeenCalledOnce()

    // A new connection after disconnecting is a first one again.
    client.disconnect()
    client.connect()
    sockets[2]!.opened()
    expect(back).toHaveBeenCalledOnce()
    off()
    sockets[2]!.dropped()
    vi.advanceTimersByTime(1000)
    sockets[3]!.opened()
    expect(back).toHaveBeenCalledOnce()
  })

  it('does not reconnect after disconnecting', () => {
    client.connect()
    client.disconnect()
    sockets[0]!.dropped()
    vi.advanceTimersByTime(60_000)
    expect(sockets).toHaveLength(1)
  })
})

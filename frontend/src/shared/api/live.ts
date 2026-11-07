import type { Job, LiveMessage, LiveRequest, LiveTopic, Metrics, ServerNotification } from './types'

/** What a topic delivers. */
interface TopicData {
  metrics: Metrics
  jobs: Job
  notifications: ServerNotification
}

type Listener<T extends LiveTopic> = (data: TopicData[T]) => void

/** The server closes with this code when the session has ended. */
const SESSION_ENDED = 4401
const MAX_RETRY_DELAY_MS = 30_000

/** The parts of a WebSocket the client uses; replaceable in tests. */
export interface SocketLike {
  readonly readyState: number
  onopen: ((event: Event) => void) | null
  onmessage: ((event: MessageEvent) => void) | null
  onclose: ((event: CloseEvent) => void) | null
  send(data: string): void
  close(): void
}

/**
 * The one WebSocket of this tab. Topics are subscribed while someone
 * listens; after a dropped connection it reconnects with growing delays and
 * subscribes again.
 */
export class LiveClient {
  private socket: SocketLike | null = null
  private readonly listeners = new Map<LiveTopic, Set<Listener<LiveTopic>>>()
  private readonly reconnectListeners = new Set<() => void>()
  private wanted = false
  /** Whether a socket of this connection was open before: the next one is a reconnect. */
  private wasOpen = false
  private retries = 0
  private retryTimer: ReturnType<typeof setTimeout> | undefined

  /** Called when the server ends the connection because the session ended. */
  onSessionEnded: () => void = () => undefined

  constructor(
    private readonly url: () => string,
    private readonly open: (url: string) => SocketLike = (url) => new WebSocket(url),
  ) {}

  connect(): void {
    this.wanted = true
    if (!this.socket) this.dial()
  }

  disconnect(): void {
    this.wanted = false
    this.wasOpen = false
    clearTimeout(this.retryTimer)
    this.socket?.close()
    this.socket = null
  }

  /** Listens to `topic` until the returned function is called. */
  subscribe<T extends LiveTopic>(topic: T, listener: Listener<T>): () => void {
    let set = this.listeners.get(topic)
    if (!set) {
      set = new Set()
      this.listeners.set(topic, set)
      this.send({ type: 'subscribe', topic })
    }
    set.add(listener as Listener<LiveTopic>)
    return () => {
      set.delete(listener as Listener<LiveTopic>)
      if (set.size === 0) {
        this.listeners.delete(topic)
        this.send({ type: 'unsubscribe', topic })
      }
    }
  }

  /**
   * Calls `listener` whenever the connection is back after it dropped:
   * what happened in between was not delivered, so lists are to be loaded
   * again. Until the returned function is called.
   */
  onReconnect(listener: () => void): () => void {
    this.reconnectListeners.add(listener)
    return () => this.reconnectListeners.delete(listener)
  }

  private dial(): void {
    const socket = this.open(this.url())
    this.socket = socket
    socket.onopen = () => {
      this.retries = 0
      for (const topic of this.listeners.keys()) this.send({ type: 'subscribe', topic })
      if (this.wasOpen) for (const listener of this.reconnectListeners) listener()
      this.wasOpen = true
    }
    socket.onmessage = (event) => this.dispatch(event.data)
    socket.onclose = (event) => {
      this.socket = null
      if (event.code === SESSION_ENDED) {
        this.wanted = false
        this.onSessionEnded()
      } else if (this.wanted) {
        const delay = Math.min(1000 * 2 ** this.retries, MAX_RETRY_DELAY_MS)
        this.retries += 1
        this.retryTimer = setTimeout(() => this.dial(), delay)
      }
    }
  }

  private send(request: LiveRequest): void {
    if (this.socket?.readyState === 1) this.socket.send(JSON.stringify(request))
  }

  private dispatch(data: unknown): void {
    if (typeof data !== 'string') return
    let message: LiveMessage
    try {
      message = JSON.parse(data) as LiveMessage
    } catch {
      return
    }
    if (message.type === 'metrics') {
      for (const listener of this.listeners.get('metrics') ?? []) listener(message.data)
    } else if (message.type === 'job') {
      for (const listener of this.listeners.get('jobs') ?? []) listener(message.data)
    } else if (message.type === 'notification') {
      for (const listener of this.listeners.get('notifications') ?? []) listener(message.data)
    }
  }
}

export const live = new LiveClient(
  () =>
    `${window.location.protocol === 'https:' ? 'wss' : 'ws'}://${window.location.host}/api/v1/ws`,
)

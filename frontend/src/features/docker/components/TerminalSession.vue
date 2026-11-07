<script setup lang="ts">
import '@xterm/xterm/css/xterm.css'

import { FitAddon } from '@xterm/addon-fit'
import { Terminal } from '@xterm/xterm'
import { nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import { terminalUrl } from '../api'

/** `active`: shown, so it takes the keyboard and its size. */
const props = defineProps<{ container: string; command: string; active: boolean }>()
const emit = defineEmits<{ state: [state: 'open' | 'ended' | 'failed', reason?: string] }>()

const { t } = useI18n()
const box = ref<HTMLElement | null>(null)
let terminal: Terminal | null = null
let fit: FitAddon | null = null
let socket: WebSocket | null = null
let observer: ResizeObserver | null = null
const encoder = new TextEncoder()

/** A line of zendiloOS's own in the terminal, dimmed. */
function note(text: string): void {
  terminal?.write(`\r\n\x1b[2m${text}\x1b[0m\r\n`)
}

function resize(): void {
  if (!fit || !terminal || !props.active) return
  fit.fit()
}

onMounted(() => {
  terminal = new Terminal({
    cursorBlink: true,
    fontFamily: 'JetBrains Mono Variable, ui-monospace, monospace',
    fontSize: 13,
    theme: { background: '#18181b' },
  })
  fit = new FitAddon()
  terminal.loadAddon(fit)
  terminal.open(box.value!)
  resize()
  socket = new WebSocket(terminalUrl(props.container, props.command, terminal.cols, terminal.rows))
  socket.binaryType = 'arraybuffer'
  socket.onopen = () => emit('state', 'open')
  socket.onmessage = (event: MessageEvent<ArrayBuffer | string>) => {
    if (typeof event.data !== 'string') {
      terminal?.write(new Uint8Array(event.data))
      return
    }
    const message = JSON.parse(event.data) as
      { type: 'ended'; reason: string } | { type: 'error'; message: string }
    if (message.type === 'ended') {
      note(t(`docker.terminal.ended.${message.reason}`))
      emit('state', 'ended', message.reason)
    } else {
      note(message.message)
      emit('state', 'failed', message.message)
    }
  }
  socket.onclose = () => emit('state', 'ended')
  terminal.onData((data) => {
    if (socket?.readyState === WebSocket.OPEN) socket.send(encoder.encode(data))
  })
  terminal.onResize(({ cols, rows }) => {
    if (socket?.readyState === WebSocket.OPEN)
      socket.send(JSON.stringify({ type: 'resize', columns: cols, rows }))
  })
  observer = new ResizeObserver(resize)
  observer.observe(box.value!)
  if (props.active) terminal.focus()
})

watch(
  () => props.active,
  async (active) => {
    if (!active) return
    await nextTick()
    resize()
    terminal?.focus()
  },
)

onBeforeUnmount(() => {
  observer?.disconnect()
  // Closing the socket hangs the terminal up, which ends its program.
  socket?.close()
  terminal?.dispose()
})
</script>

<!-- One terminal in a container: what is typed goes in, what the program writes comes out. -->
<template>
  <div ref="box" class="h-full min-h-80 overflow-hidden rounded-lg bg-zinc-900 p-2" />
</template>

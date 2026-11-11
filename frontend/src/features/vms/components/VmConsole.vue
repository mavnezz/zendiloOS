<script setup lang="ts">
import { Expand, Keyboard, LoaderCircle, RefreshCw } from 'lucide-vue-next'
import { onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type RFB from '@novnc/novnc'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'

import { consoleUrl } from '../api'

/** A running VM's screen, keyboard and mouse, through noVNC. */
const props = defineProps<{ vm: string }>()

const { t } = useI18n()
const frame = ref<HTMLElement | null>(null)
const screen = ref<HTMLElement | null>(null)
const state = ref<'connecting' | 'connected' | 'closed'>('connecting')
/** Why it closed, as the server said in the close frame. */
const reason = ref<string | null>(null)
let rfb: RFB | null = null

/**
 * noVNC speaks RFB over a WebSocket made here: the server says why it
 * closes in the close frame, which noVNC does not pass on.
 */
async function connect(): Promise<void> {
  disconnect()
  state.value = 'connecting'
  reason.value = null
  const target = screen.value
  if (!target) return
  const { default: Client } = await import('@novnc/novnc')
  const socket = new WebSocket(consoleUrl(props.vm), ['binary'])
  socket.addEventListener('close', (event) => {
    // 1000: the console ended (the VM stopped); 4000 and up: zendiloOS's reasons.
    if (event.code >= 4000 && event.reason) reason.value = event.reason
  })
  const client = new Client(target, socket, { shared: true })
  client.scaleViewport = true
  client.resizeSession = false
  client.background = 'transparent'
  client.addEventListener('connect', () => {
    state.value = 'connected'
    client.focus()
  })
  client.addEventListener('disconnect', () => {
    state.value = 'closed'
    if (rfb === client) rfb = null
  })
  rfb = client
}

function disconnect(): void {
  const client = rfb
  rfb = null
  try {
    client?.disconnect()
  } catch {
    // Closed already.
  }
}

function ctrlAltDel(): void {
  rfb?.sendCtrlAltDel()
  rfb?.focus()
}

async function fullScreen(): Promise<void> {
  try {
    await frame.value?.requestFullscreen()
  } catch {
    // The browser refused; nothing to do.
  }
  rfb?.focus()
}

onMounted(connect)
watch(() => props.vm, connect)
onBeforeUnmount(disconnect)
</script>

<template>
  <div ref="frame" class="flex h-full min-h-0 flex-col bg-zinc-950">
    <div class="flex flex-wrap items-center gap-2 border-b border-white/10 bg-zinc-900 px-3 py-2">
      <span class="mr-auto flex items-center gap-2 text-sm text-zinc-300">
        <LoaderCircle v-if="state === 'connecting'" class="size-4 animate-spin" />
        <template v-if="state === 'connecting'">{{ t('vms.consoleWindow.connecting') }}</template>
      </span>
      <Button size="sm" variant="secondary" :disabled="state !== 'connected'" @click="ctrlAltDel">
        <Keyboard />
        {{ t('vms.consoleWindow.ctrlAltDel') }}
      </Button>
      <Button size="sm" variant="secondary" :disabled="state !== 'connected'" @click="fullScreen">
        <Expand />
        {{ t('vms.consoleWindow.fullscreen') }}
      </Button>
    </div>
    <div class="relative min-h-0 flex-1">
      <div ref="screen" class="absolute inset-0" data-testid="vm-screen" />
      <div
        v-if="state === 'closed'"
        class="absolute inset-0 grid place-content-center justify-items-center gap-3 p-6 text-center"
      >
        <Alert class="max-w-sm">{{ reason ?? t('vms.consoleWindow.closed') }}</Alert>
        <Button size="sm" @click="connect">
          <RefreshCw />
          {{ t('vms.consoleWindow.reconnect') }}
        </Button>
      </div>
    </div>
  </div>
</template>

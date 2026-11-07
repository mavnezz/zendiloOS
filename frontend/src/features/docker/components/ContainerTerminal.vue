<script setup lang="ts">
import { Plus, SquareTerminal, X } from 'lucide-vue-next'
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { Button } from '@/shared/ui/button'
import { ModalDialog } from '@/shared/ui/dialog'
import { Input } from '@/shared/ui/input'
import { Label } from '@/shared/ui/label'

import TerminalSession from './TerminalSession.vue'

/** `container`: its ID; `running`: only then can a terminal be opened. */
const props = defineProps<{ container: string; running: boolean }>()

const { t } = useI18n()
/** Programs most images have. */
const SUGGESTED = ['/bin/sh', '/bin/bash']

interface Session {
  key: number
  command: string
  state: 'connecting' | 'open' | 'ended' | 'failed'
}
const sessions = ref<Session[]>([])
const active = ref<number | null>(null)
let next = 1

const adding = ref(false)
const command = ref(SUGGESTED[0]!)
const commandValid = computed(() => {
  const words = command.value.trim().split(/\s+/)
  return command.value.trim() !== '' && words.length <= 16 && !words[0]!.startsWith('-')
})

function startAdding(): void {
  command.value = SUGGESTED[0]!
  adding.value = true
}

function add(): void {
  if (!commandValid.value) return
  const session: Session = { key: next++, command: command.value.trim(), state: 'connecting' }
  sessions.value.push(session)
  active.value = session.key
  adding.value = false
}

function end(key: number): void {
  sessions.value = sessions.value.filter((session) => session.key !== key)
  if (active.value === key) active.value = sessions.value.at(-1)?.key ?? null
}

function changed(key: number, state: Session['state']): void {
  const session = sessions.value.find((candidate) => candidate.key === key)
  // An error said first stays, though the socket closes after it.
  if (session && session.state !== 'failed') session.state = state
}

const dot = (state: Session['state']) =>
  state === 'open'
    ? 'bg-success'
    : state === 'connecting'
      ? 'bg-warning'
      : state === 'failed'
        ? 'bg-destructive'
        : 'bg-muted-foreground'
</script>

<!-- Terminals in a running container: several at once, each its own program. -->
<template>
  <div class="grid min-h-96 gap-4 @3xl:grid-cols-[14rem_1fr]">
    <div class="grid content-start gap-2">
      <Button size="sm" :disabled="!running" @click="startAdding">
        <Plus />
        {{ t('docker.terminal.add') }}
      </Button>
      <p v-if="!running" class="text-xs text-muted-foreground">
        {{ t('docker.terminal.notRunning') }}
      </p>
      <ul class="grid gap-1" :aria-label="t('docker.terminal.sessions')">
        <li
          v-for="session in sessions"
          :key="session.key"
          class="flex items-center gap-2 rounded-md px-2 py-1.5 text-sm"
          :class="active === session.key ? 'bg-primary/10 text-primary' : 'hover:bg-accent'"
        >
          <span :class="['size-2 shrink-0 rounded-full', dot(session.state)]" aria-hidden="true" />
          <button
            type="button"
            class="min-w-0 flex-1 truncate text-left font-mono text-xs"
            @click="active = session.key"
          >
            {{ session.command }}
          </button>
          <button
            type="button"
            class="rounded p-0.5 text-muted-foreground hover:text-destructive"
            :aria-label="t('docker.terminal.end', { command: session.command })"
            @click="end(session.key)"
          >
            <X class="size-3.5" />
          </button>
        </li>
      </ul>
    </div>
    <div class="min-w-0">
      <div
        v-if="!sessions.length"
        class="grid h-full min-h-80 place-items-center rounded-lg border border-dashed text-center text-sm text-muted-foreground"
      >
        <div class="grid justify-items-center gap-2 p-6">
          <SquareTerminal class="size-8" />
          <p>{{ t('docker.terminal.none') }}</p>
        </div>
      </div>
      <TerminalSession
        v-for="session in sessions"
        v-show="active === session.key"
        :key="session.key"
        :container="props.container"
        :command="session.command"
        :active="active === session.key"
        class="h-[60vh]"
        @state="(state) => changed(session.key, state)"
      />
    </div>

    <ModalDialog
      v-model:open="adding"
      :title="t('docker.terminal.addTitle')"
      :close-label="t('desktop.close')"
    >
      <form id="terminal-command" class="grid gap-2" @submit.prevent="add">
        <Label for="terminal-command-input">{{ t('docker.terminal.command') }}</Label>
        <Input
          id="terminal-command-input"
          v-model="command"
          class="font-mono"
          autocomplete="off"
          spellcheck="false"
          :aria-invalid="!commandValid"
        />
        <div class="flex flex-wrap gap-1">
          <Button
            v-for="suggested in SUGGESTED"
            :key="suggested"
            type="button"
            variant="outline"
            size="sm"
            class="font-mono"
            @click="command = suggested"
          >
            {{ suggested }}
          </Button>
        </div>
        <p class="text-xs text-muted-foreground">{{ t('docker.terminal.commandHint') }}</p>
      </form>
      <template #footer>
        <Button variant="ghost" @click="adding = false">{{ t('desktop.cancel') }}</Button>
        <Button type="submit" form="terminal-command" :disabled="!commandValid">
          {{ t('docker.terminal.open') }}
        </Button>
      </template>
    </ModalDialog>
  </div>
</template>

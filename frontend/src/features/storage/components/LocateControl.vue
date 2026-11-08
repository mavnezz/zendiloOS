<script setup lang="ts">
import { Lightbulb, LightbulbOff } from 'lucide-vue-next'
import { computed, onUnmounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import { problemMessage } from '@/shared/i18n'
import { Button } from '@/shared/ui/button'

import { startLocate, stopLocate, useLocating, useRefreshStorage } from '../api'
import { formatCountdown, secondsLeft } from '../lib/enclosure'

/** `canManage`: may make it blink (`storage.manage`). */
const props = defineProps<{ disk: string; nvme?: boolean; canManage: boolean }>()

const { t, locale } = useI18n()
const { data: locating, refetch } = useLocating()
const refresh = useRefreshStorage()

const session = computed(() => locating.value?.find((candidate) => candidate.disk === props.disk))
const now = ref(Date.now())
const failure = ref<string | null>(null)
const pending = ref(false)

// A countdown each second while it blinks; when it is up, the list is asked again.
let timer: ReturnType<typeof setInterval> | undefined
watch(
  session,
  (current) => {
    clearInterval(timer)
    timer = undefined
    if (!current) return
    now.value = Date.now()
    timer = setInterval(() => {
      now.value = Date.now()
      if (secondsLeft(current.until, now.value) === 0) void refetch()
    }, 1000)
  },
  { immediate: true },
)
onUnmounted(() => clearInterval(timer))

const left = computed(() => (session.value ? secondsLeft(session.value.until, now.value) : 0))
const until = computed(() =>
  session.value ? new Date(session.value.until * 1000).toLocaleTimeString(locale.value) : '',
)

async function act(action: () => Promise<unknown>): Promise<void> {
  failure.value = null
  pending.value = true
  try {
    await action()
    await refresh()
  } catch (problem) {
    failure.value = problemMessage(problem)
  } finally {
    pending.value = false
  }
}

const start = () => act(() => startLocate(props.disk))
const stop = () => act(() => stopLocate(props.disk))
</script>

<!-- Makes a disk's activity light blink for a minute, so it can be found in its slot. -->
<template>
  <section class="grid gap-1.5">
    <div class="flex flex-wrap items-center gap-2">
      <p v-if="session" class="min-w-0 flex-1 text-sm" role="status">
        <span class="font-medium text-primary">{{
          t('storage.locate.until', { time: until })
        }}</span>
        <span class="text-muted-foreground tabular-nums">
          · {{ t('storage.locate.left', { left: formatCountdown(left) }) }}
        </span>
      </p>
      <p v-else class="min-w-0 flex-1 text-xs text-muted-foreground">
        {{ t('storage.locate.hint') }}
        <template v-if="nvme"> {{ t('storage.locate.nvmeHint') }}</template>
      </p>
      <template v-if="canManage">
        <Button v-if="session" variant="outline" size="sm" :disabled="pending" @click="stop">
          <LightbulbOff />
          {{ t('storage.locate.stop') }}
        </Button>
        <Button v-else variant="outline" size="sm" :disabled="pending" @click="start">
          <Lightbulb />
          {{ t('storage.locate.button') }}
        </Button>
      </template>
    </div>
    <p v-if="failure" class="text-xs text-destructive">{{ failure }}</p>
  </section>
</template>

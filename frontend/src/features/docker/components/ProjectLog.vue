<script setup lang="ts">
import { Download, Search } from 'lucide-vue-next'
import { computed, nextTick, ref, toRef, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import { problemMessage } from '@/shared/i18n'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { Input } from '@/shared/ui/input'
import { NativeSelect } from '@/shared/ui/native-select'
import { Switch } from '@/shared/ui/switch'

import { useProjectLog } from '../api'
import { logColors, matching, parseProjectLog } from '../lib/projectLog'

/** `containers`: the project's, for their colours. */
const props = defineProps<{ name: string; containers: string[] }>()

const { t } = useI18n()
const follow = ref(true)
const count = ref(500)
const query = ref('')
const { data, error, isPending } = useProjectLog(toRef(props, 'name'), count, follow)

const lines = computed(() => parseProjectLog(data.value?.text ?? ''))
const colors = computed(() => logColors(props.containers, lines.value))
const shown = computed(() => matching(lines.value, query.value))
const box = ref<HTMLElement | null>(null)

// While followed, new lines come at the end: stay there, unless the reader scrolled up.
watch(
  () => data.value?.text,
  async () => {
    if (!follow.value) return
    const element = box.value
    const atEnd = !element || element.scrollHeight - element.scrollTop - element.clientHeight < 40
    await nextTick()
    if (atEnd && box.value) box.value.scrollTop = box.value.scrollHeight
  },
  { immediate: true },
)

/** The log as read, as a text file. */
function download(): void {
  const text = data.value?.text ?? ''
  const url = URL.createObjectURL(new Blob([text], { type: 'text/plain;charset=utf-8' }))
  const link = document.createElement('a')
  link.href = url
  link.download = `${props.name}.log`
  link.click()
  URL.revokeObjectURL(url)
}
</script>

<!-- All of a project's containers' lines together, each under its container in its colour. -->
<template>
  <div class="grid gap-3">
    <div class="flex flex-wrap items-center gap-3">
      <label class="flex items-center gap-2 text-sm">
        <Switch v-model="follow" :aria-label="t('docker.projectLog.follow')" />
        {{ t('docker.projectLog.follow') }}
      </label>
      <NativeSelect v-model.number="count" class="w-36" :aria-label="t('docker.projectLog.lines')">
        <option :value="200">{{ t('docker.projectLog.last', { n: 200 }) }}</option>
        <option :value="500">{{ t('docker.projectLog.last', { n: 500 }) }}</option>
        <option :value="2000">{{ t('docker.projectLog.last', { n: 2000 }) }}</option>
      </NativeSelect>
      <div class="relative ml-auto w-56">
        <Search
          class="pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted-foreground"
        />
        <Input
          v-model="query"
          type="search"
          class="pl-8"
          :placeholder="t('docker.projectLog.search')"
          :aria-label="t('docker.projectLog.search')"
        />
      </div>
      <Button
        variant="ghost"
        size="sm"
        :disabled="!data?.text"
        :aria-label="t('docker.projectLog.download')"
        :title="t('docker.projectLog.download')"
        @click="download"
      >
        <Download />
      </Button>
    </div>
    <Alert v-if="error" variant="destructive">{{ problemMessage(error) }}</Alert>
    <div
      ref="box"
      role="log"
      :aria-label="t('docker.projects.log')"
      class="max-h-[60vh] min-h-48 overflow-auto rounded-lg bg-zinc-900 p-3 font-mono text-xs leading-5 text-zinc-100"
    >
      <p v-if="isPending" class="text-zinc-400">{{ t('docker.logs.loading') }}</p>
      <p v-else-if="!shown.length" class="text-zinc-400">
        {{ query.trim() ? t('docker.projectLog.noMatch') : t('docker.logs.none') }}
      </p>
      <div v-for="(line, index) in shown" :key="index" class="break-all whitespace-pre-wrap">
        <span
          v-if="line.container"
          class="mr-2 select-none"
          :class="colors.get(line.container)"
          data-testid="log-container"
          >{{ line.container }}</span
        >{{ line.text }}
      </div>
    </div>
    <p v-if="query.trim() && lines.length" class="text-xs text-muted-foreground">
      {{ t('docker.projectLog.matches', { shown: shown.length, all: lines.length }) }}
    </p>
  </div>
</template>

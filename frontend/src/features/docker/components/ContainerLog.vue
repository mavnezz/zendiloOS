<script setup lang="ts">
import { Download, Search } from 'lucide-vue-next'
import { computed, ref, toRef } from 'vue'
import { useI18n } from 'vue-i18n'

import { Button } from '@/shared/ui/button'
import { Input } from '@/shared/ui/input'
import { NativeSelect } from '@/shared/ui/native-select'
import { Switch } from '@/shared/ui/switch'

import { useContainerLog } from '../api'
import LogView from './LogView.vue'

/** `id`: the container's; `name`: for the file a download saves. */
const props = defineProps<{ id: string; name: string }>()
const { t } = useI18n()
const live = ref(true)
const lines = ref(500)
const filter = ref('')
const { data, isPending } = useContainerLog(toRef(props, 'id'), lines, live)

/** The lines that contain what is searched, case aside; all without a search. */
const shown = computed(() => {
  const text = data.value?.text ?? ''
  const term = filter.value.trim().toLowerCase()
  if (!term) return text
  return text
    .split('\n')
    .filter((line) => line.toLowerCase().includes(term))
    .join('\n')
})

function download(): void {
  const url = URL.createObjectURL(new Blob([data.value?.text ?? ''], { type: 'text/plain' }))
  const link = document.createElement('a')
  link.href = url
  link.download = `${props.name}.log`
  link.click()
  URL.revokeObjectURL(url)
}
</script>

<!-- What a container wrote, followed while it runs. -->
<template>
  <div class="grid gap-3">
    <div class="flex flex-wrap items-center gap-3">
      <label class="flex items-center gap-2 text-sm">
        <Switch v-model="live" :aria-label="t('docker.logs.live')" />
        {{ t('docker.logs.live') }}
      </label>
      <NativeSelect v-model.number="lines" class="w-36" :aria-label="t('docker.logs.lines')">
        <option v-for="count in [200, 500, 2000, 10000]" :key="count" :value="count">
          {{ t('docker.logs.lastLines', { n: count }) }}
        </option>
      </NativeSelect>
      <div class="relative ml-auto w-full max-w-60">
        <Search class="absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted-foreground" />
        <Input
          v-model="filter"
          class="pl-8"
          :placeholder="t('docker.search')"
          :aria-label="t('docker.logs.search')"
        />
      </div>
      <Button variant="outline" size="sm" :disabled="!data?.text" @click="download">
        <Download />
        {{ t('docker.logs.download') }}
      </Button>
    </div>
    <LogView :text="shown" :loading="isPending" />
  </div>
</template>

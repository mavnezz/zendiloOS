<script setup lang="ts">
import { CircleCheck, CircleX, LoaderCircle } from 'lucide-vue-next'
import { computed, nextTick, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { Certificate } from '@/shared/api/types'
import { useJobsStore } from '@/shared/jobs/store'
import { Button } from '@/shared/ui/button'
import { ModalDialog } from '@/shared/ui/dialog'

/**
 * What acme.sh writes while it issues a certificate (the job's output, as
 * it comes), or what it wrote at the last try when no job is followed.
 */
const props = defineProps<{ certificate: Certificate | null; jobId: number | null }>()
const open = defineModel<boolean>('open', { default: false })

const { t } = useI18n()
const jobs = useJobsStore()
const job = computed(() => (props.jobId === null ? undefined : jobs.jobs.get(props.jobId)))
const running = computed(
  () => props.jobId !== null && (!job.value || job.value.state === 'running'),
)
const lines = computed(() =>
  props.jobId !== null ? (job.value?.output ?? []) : (props.certificate?.last_output ?? []),
)
const succeeded = computed(() =>
  props.jobId !== null ? job.value?.state === 'succeeded' : !props.certificate?.last_error,
)
const reason = computed(() =>
  props.jobId !== null ? job.value?.error_detail : props.certificate?.last_error,
)
const box = ref<HTMLElement | null>(null)

// New lines come at the end: stay there, unless the reader scrolled up.
watch(
  () => lines.value.length,
  async () => {
    const element = box.value
    const atEnd = !element || element.scrollHeight - element.scrollTop - element.clientHeight < 40
    await nextTick()
    if (atEnd && box.value) box.value.scrollTop = box.value.scrollHeight
  },
)
</script>

<!-- acme.sh's lines for one certificate. Text, never markup. -->
<template>
  <ModalDialog
    v-model:open="open"
    wide
    :title="t('appStore.acme.logTitle')"
    :description="certificate?.domains.join(', ')"
    :close-label="t('desktop.close')"
  >
    <pre
      ref="box"
      data-testid="acme-output"
      class="h-72 overflow-auto rounded-lg bg-zinc-900 p-3 font-mono text-[12px] leading-5 whitespace-pre-wrap text-zinc-100"
      >{{
        lines.length
          ? lines.join('\n')
          : running
            ? t('appStore.acme.logWaiting')
            : t('appStore.acme.logNone')
      }}</pre>
    <p
      v-if="running || lines.length || reason"
      class="flex items-center gap-2 text-sm"
      role="status"
    >
      <template v-if="running">
        <LoaderCircle class="size-4 animate-spin text-primary" />
        {{ t('appStore.acme.logRunning') }}
      </template>
      <template v-else-if="succeeded">
        <CircleCheck class="size-4 text-success" />
        {{ t('appStore.acme.logSucceeded') }}
      </template>
      <template v-else>
        <CircleX class="size-4 text-destructive" />
        <span>
          {{ t('appStore.acme.logFailed') }}
          <span v-if="reason" class="block font-mono text-xs text-muted-foreground">{{
            reason
          }}</span>
        </span>
      </template>
    </p>
    <template #footer>
      <Button v-if="running" variant="ghost" @click="open = false">{{
        t('appStore.acme.hide')
      }}</Button>
      <Button v-else @click="open = false">{{ t('docker.deploy.ok') }}</Button>
    </template>
  </ModalDialog>
</template>

<script setup lang="ts">
import { CircleCheck, CircleX, LoaderCircle } from 'lucide-vue-next'
import { computed, nextTick, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import { jobTitle, useJobsStore } from '@/shared/jobs/store'
import { Button } from '@/shared/ui/button'
import { ModalDialog } from '@/shared/ui/dialog'

/** The job deploying a project; its output comes over the live socket. */
const props = defineProps<{ jobId: number | null }>()
const open = defineModel<boolean>('open', { default: false })
const emit = defineEmits<{ done: [succeeded: boolean] }>()

const { t } = useI18n()
const jobs = useJobsStore()
const job = computed(() => (props.jobId === null ? undefined : jobs.jobs.get(props.jobId)))
const running = computed(() => !job.value || job.value.state === 'running')
const lines = computed(() => job.value?.output ?? [])
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

/** Closing while it runs leaves the job running; it is announced when it ends. */
function close(): void {
  open.value = false
}

// Closed once the job ended, by OK or otherwise: what it came to.
watch(open, (isOpen) => {
  if (!isOpen && job.value && !running.value) emit('done', job.value.state === 'succeeded')
})
</script>

<!-- What Docker Compose writes while a project is deployed, as it comes. -->
<template>
  <ModalDialog
    v-model:open="open"
    wide
    :title="t('docker.deploy.title')"
    :description="job ? jobTitle(job) : undefined"
    :close-label="t('desktop.close')"
  >
    <pre
      ref="box"
      data-testid="deploy-output"
      class="h-72 overflow-auto rounded-lg bg-zinc-900 p-3 font-mono text-[12px] leading-5 whitespace-pre-wrap text-zinc-100"
      >{{ lines.length ? lines.join('\n') : t('docker.deploy.waiting') }}</pre>
    <p class="flex items-center gap-2 text-sm" role="status">
      <template v-if="running">
        <LoaderCircle class="size-4 animate-spin text-primary" />
        {{ t('docker.deploy.running') }}
      </template>
      <template v-else-if="job?.state === 'succeeded'">
        <CircleCheck class="size-4 text-success" />
        {{ t('docker.deploy.succeeded') }}
      </template>
      <template v-else>
        <CircleX class="size-4 text-destructive" />
        <span>
          {{ t('docker.deploy.failed') }}
          <span v-if="job?.error_detail" class="block font-mono text-xs text-muted-foreground">{{
            job.error_detail
          }}</span>
        </span>
      </template>
    </p>
    <template #footer>
      <Button v-if="running" variant="ghost" @click="close">{{ t('docker.deploy.hide') }}</Button>
      <Button v-else @click="close">{{ t('docker.deploy.ok') }}</Button>
    </template>
  </ModalDialog>
</template>

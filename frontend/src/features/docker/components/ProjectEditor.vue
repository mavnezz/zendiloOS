<script setup lang="ts">
import { ArrowLeft, FileUp, Rocket, WandSparkles } from 'lucide-vue-next'
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { ComposeRisk, Job } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { useJobsStore } from '@/shared/jobs/store'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { CodeEditor } from '@/shared/ui/code-editor'
import { Input } from '@/shared/ui/input'
import { Label } from '@/shared/ui/label'
import { NativeSelect } from '@/shared/ui/native-select'
import { RisksDialog } from '@/shared/ui/risks-dialog'

import { checkCompose, useChangeCompose, useCreateProject, usePlaces } from '../api'
import { DeployLogDialog } from '@/shared/ui/deploy-log'

/** Without `name`, a new project; with it, that project's file to change. */
const props = defineProps<{ name?: string; compose?: string }>()
const emit = defineEmits<{ back: []; done: [name: string] }>()

const NAME = /^[a-z0-9][a-z0-9_-]{0,62}$/
const EXAMPLE = `services:
  app:
    image: nginx:latest
    restart: unless-stopped
    ports:
      - "8080:80"
    volumes:
      - ./html:/usr/share/nginx/html
    environment:
      - TZ=Europe/Berlin
`

const { t } = useI18n()
const editing = computed(() => props.name !== undefined)
const { data: places } = usePlaces()
const create = useCreateProject()
const change = useChangeCompose()

const name = ref(props.name ?? '')
const shareId = ref<number>(0)
const folder = ref('')
/** The folder follows the name until it is typed in itself. */
const folderTouched = ref(false)
const compose = ref(props.compose ?? '')
const start = ref(true)
/** Changing it: the newest images under its tags first, as a redeploy may. */
const pull = ref(false)
const checking = ref(false)
const failure = ref<unknown>(null)
const risks = ref<ComposeRisk[]>([])
/** What the check came to: accepting its risks holds for that only. */
const digest = ref<string | null>(null)
const confirming = ref(false)

watch(
  places,
  (list) => {
    if (!shareId.value && list?.length) shareId.value = list[0]!.share_id
  },
  { immediate: true },
)
watch(name, (value) => {
  if (!folderTouched.value) folder.value = value ? `docker/${value}` : ''
})

const nameValid = computed(() => NAME.test(name.value))
const ready = computed(
  () =>
    compose.value.trim() !== '' &&
    (editing.value || (nameValid.value && shareId.value > 0 && folder.value.trim() !== '')),
)
const busy = computed(() => checking.value || create.isPending.value || change.isPending.value)

async function load(event: Event): Promise<void> {
  const file = (event.target as HTMLInputElement).files?.[0]
  if (file) compose.value = await file.text()
}

/** Checks the file; asks about risks before anything runs. */
async function deploy(): Promise<void> {
  failure.value = null
  checking.value = true
  try {
    const result = await checkCompose(
      editing.value
        ? { name: name.value, compose: compose.value }
        : {
            name: name.value,
            compose: compose.value,
            share_id: shareId.value,
            folder: folder.value,
          },
    )
    risks.value = result.risks
    digest.value = result.digest
    if (result.risks.length) confirming.value = true
    else submit(false)
  } catch (error) {
    failure.value = error
  } finally {
    checking.value = false
  }
}

/** The job deploying it, shown as it runs; the project's page once closed. */
const jobs = useJobsStore()
const deploying = ref<number | null>(null)
const showing = ref(false)
function deployed(job: Job | null | undefined): void {
  if (!job) {
    emit('done', name.value)
    return
  }
  jobs.update(job)
  deploying.value = job.id
  showing.value = true
}
watch(showing, (open) => !open && deploying.value !== null && emit('done', name.value))

function submit(accept: boolean): void {
  const failed = (error: unknown) => (failure.value = error)
  const accepted = accept ? digest.value : null
  if (editing.value) {
    change.mutate(
      { name: name.value, compose: compose.value, accepted, pull: pull.value },
      { onSuccess: deployed, onError: failed },
    )
  } else {
    create.mutate(
      {
        name: name.value,
        share_id: shareId.value,
        folder: folder.value,
        compose: compose.value,
        start: start.value,
        accepted,
      },
      { onSuccess: (made) => deployed(made.job), onError: failed },
    )
  }
}
</script>

<!-- A Compose project: its name, its folder in a share and its file. -->
<template>
  <div class="flex min-h-full flex-col gap-4">
    <div class="flex items-center gap-2">
      <Button variant="ghost" size="icon" :aria-label="t('docker.back')" @click="emit('back')">
        <ArrowLeft />
      </Button>
      <h2 class="text-lg font-semibold">
        {{ editing ? t('docker.editor.editTitle', { name }) : t('docker.editor.createTitle') }}
      </h2>
    </div>

    <div v-if="!editing" class="grid max-w-3xl gap-4 @2xl:grid-cols-2">
      <div class="grid gap-1.5">
        <Label for="project-name">{{ t('docker.editor.name') }}</Label>
        <Input
          id="project-name"
          v-model="name"
          class="font-mono text-[13px]"
          autocomplete="off"
          spellcheck="false"
          :aria-invalid="name !== '' && !nameValid"
        />
        <p
          class="text-xs"
          :class="name !== '' && !nameValid ? 'text-destructive' : 'text-muted-foreground'"
        >
          {{ t('docker.editor.nameHint') }}
        </p>
      </div>
      <div class="grid gap-1.5">
        <Label for="project-folder">{{ t('docker.editor.folder') }}</Label>
        <div class="flex gap-2">
          <NativeSelect
            v-model.number="shareId"
            class="w-40"
            :aria-label="t('docker.editor.share')"
          >
            <option v-for="place in places" :key="place.share_id" :value="place.share_id">
              {{ place.share }}
            </option>
          </NativeSelect>
          <Input
            id="project-folder"
            v-model="folder"
            class="flex-1 font-mono text-[13px]"
            autocomplete="off"
            spellcheck="false"
            @input="folderTouched = true"
          />
        </div>
        <p class="text-xs text-muted-foreground">
          {{
            places && !places.length ? t('docker.editor.noShare') : t('docker.editor.folderHint')
          }}
        </p>
      </div>
    </div>

    <div class="grid gap-2">
      <div class="flex flex-wrap items-center gap-2">
        <Label>{{ t('docker.editor.compose') }}</Label>
        <Button variant="ghost" size="sm" class="ml-auto" @click="compose = EXAMPLE">
          <WandSparkles />
          {{ t('docker.editor.example') }}
        </Button>
        <Button variant="ghost" size="sm" as="label" class="cursor-pointer">
          <FileUp />
          {{ t('docker.editor.import') }}
          <input type="file" accept=".yml,.yaml,text/yaml" class="sr-only" @change="load" />
        </Button>
      </div>
      <CodeEditor v-model="compose" :label="t('docker.editor.compose')" :min-rows="18" />
      <p class="text-xs text-muted-foreground">{{ t('docker.editor.composeHint') }}</p>
    </div>

    <Alert v-if="failure" variant="destructive" class="whitespace-pre-wrap">{{
      problemMessage(failure)
    }}</Alert>

    <div
      class="sticky bottom-0 mt-auto flex flex-wrap items-center gap-3 border-t bg-background/95 py-3"
    >
      <label v-if="!editing" class="flex items-center gap-2 text-sm">
        <input v-model="start" type="checkbox" class="size-4 accent-primary" />
        {{ t('docker.editor.start') }}
      </label>
      <label v-else class="flex items-center gap-2 text-sm" :title="t('docker.redeploy.pullHint')">
        <input v-model="pull" type="checkbox" class="size-4 accent-primary" />
        {{ t('docker.redeploy.pull') }}
      </label>
      <Button class="ml-auto" :disabled="!ready || busy" @click="deploy">
        <Rocket />
        {{ editing ? t('docker.editor.redeploy') : t('docker.editor.deploy') }}
      </Button>
    </div>
    <RisksDialog v-model:open="confirming" :risks="risks" @confirm="submit(true)" />
    <DeployLogDialog v-model:open="showing" :job-id="deploying" />
  </div>
</template>

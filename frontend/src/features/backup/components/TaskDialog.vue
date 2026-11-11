<script setup lang="ts">
import { ArrowDownToLine, ArrowUpFromLine, HardDrive } from 'lucide-vue-next'
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { BackupPlugin, BackupTask } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { StepDialog } from '@/shared/ui/dialog'
import { PageTabs } from '@/shared/ui/page'

import { useBackupActions } from '../api'
import {
  STEP_PROBLEMS,
  type TaskForm,
  type TaskKind,
  emptyForm,
  folderName,
  formOf,
  problems,
  request,
  withKind,
} from '../lib/backup'
import { placeText, retentionText, scheduleText } from '../lib/describe'
import { useNames } from '../names'
import OptionsFields from './OptionsFields.vue'
import PlacesFields from './PlacesFields.vue'

/**
 * Makes a task in four steps (type, source and target, schedule and
 * options, preview), or changes `task` in tabs of the same parts. `taken`:
 * the other tasks' names.
 */
const props = defineProps<{ plugin: BackupPlugin; taken: string[]; task?: BackupTask }>()
const open = defineModel<boolean>('open', { default: false })

const STEPS = ['kind', 'places', 'options', 'review'] as const
const TABS = ['places', 'options', 'review'] as const
const KINDS: { kind: TaskKind; icon: typeof HardDrive }[] = [
  { kind: 'local', icon: HardDrive },
  { kind: 'push', icon: ArrowUpFromLine },
  { kind: 'pull', icon: ArrowDownToLine },
]

const { t } = useI18n()
const actions = useBackupActions()
const { shareNames, shareIds, driveLabels } = useNames()
const step = ref(0)
const tab = ref<string>('places')
const form = ref<TaskForm>(emptyForm())
const pending = ref(false)
const error = ref<string | null>(null)

watch(
  open,
  (isOpen) => {
    if (!isOpen) return
    form.value = props.task ? formOf(props.task, shareNames.value) : emptyForm()
    step.value = 0
    tab.value = 'places'
    error.value = null
  },
  { immediate: true },
)

const editing = computed(() => props.task !== undefined)
const found = computed(() => problems(form.value, props.taken))
const stepDone = computed(
  () => !STEP_PROBLEMS[step.value]!.some((problem) => found.value.includes(problem)),
)
const folder = computed(() => props.task?.folder ?? folderName(form.value.name))
const preview = computed(() => request(form.value, shareIds.value))
const reviewRows = computed(() => {
  const body = preview.value
  const server = body.server
  return [
    [t('backup.wizard.reviewRows.name'), body.name],
    [t('backup.wizard.reviewRows.kind'), t(`backup.kinds.${form.value.kind}`)],
    [
      t('backup.wizard.reviewRows.source'),
      placeText(t, body.source, server, shareNames.value, driveLabels.value),
    ],
    [
      t('backup.wizard.reviewRows.target'),
      placeText(t, body.target, server, shareNames.value, driveLabels.value),
    ],
    [t('backup.wizard.reviewRows.folder'), folder.value],
    [t('backup.wizard.reviewRows.mode'), t(`backup.modes.${body.mode.kind}`)],
    [t('backup.wizard.reviewRows.retention'), retentionText(t, body.mode)],
    [t('backup.wizard.reviewRows.schedule'), scheduleText(t, body.schedule, body.on_plug ?? false)],
    [
      t('backup.wizard.reviewRows.excludes'),
      body.excludes.join(', ') || t('backup.wizard.nothingLeftOut'),
    ],
    [
      t('backup.wizard.reviewRows.bandwidth'),
      body.bwlimit_kib == null
        ? t('backup.wizard.bandwidths.default')
        : body.bwlimit_kib === 0
          ? t('backup.wizard.bandwidths.none')
          : `${body.bwlimit_kib} ${t('backup.wizard.kibPerSecond')}`,
    ],
  ]
})

function choose(kind: TaskKind): void {
  form.value = withKind(form.value, kind)
}

function next(): void {
  if (stepDone.value && step.value < STEPS.length - 1) step.value++
}

async function save(): Promise<void> {
  if (found.value.length) return
  pending.value = true
  error.value = null
  try {
    if (props.task) await actions.update(props.task.id, preview.value)
    else await actions.create(preview.value)
    open.value = false
  } catch (problem) {
    error.value = problemMessage(problem)
  } finally {
    pending.value = false
  }
}

const current = computed(() => (editing.value ? tab.value : STEPS[step.value]))
</script>

<template>
  <StepDialog
    v-model:open="open"
    :title="task ? t('backup.wizard.editTitle', { name: task.name }) : t('backup.wizard.title')"
    :close-label="t('desktop.close')"
    :steps="editing ? undefined : STEPS.map((name) => t(`backup.wizard.steps.${name}`))"
    :step="step"
    wide
  >
    <template v-if="editing" #tabs>
      <div class="px-5">
        <PageTabs
          v-model="tab"
          :tabs="
            TABS.map((id) => ({
              id,
              label:
                id === 'review' ? t('backup.wizard.steps.review') : t(`backup.wizard.tabs.${id}`),
            }))
          "
          :label="t('backup.wizard.editTitle', { name: task?.name ?? '' })"
        />
      </div>
    </template>
    <form
      id="backup-task"
      class="grid gap-5"
      @submit.prevent="!editing && step < 3 ? next() : save()"
    >
      <div v-if="current === 'kind'" class="grid gap-3">
        <div
          class="grid gap-3 @2xl:grid-cols-3"
          role="radiogroup"
          :aria-label="t('backup.wizard.steps.kind')"
        >
          <label
            v-for="choice in KINDS"
            :key="choice.kind"
            class="grid cursor-pointer gap-2 rounded-xl border p-4 text-sm transition-colors"
            :class="form.kind === choice.kind && 'border-primary bg-primary/5 dark:bg-primary/10'"
            :data-kind="choice.kind"
          >
            <span class="flex items-center gap-2 font-medium">
              <input
                type="radio"
                name="backup-kind"
                class="sr-only"
                :checked="form.kind === choice.kind"
                @change="choose(choice.kind)"
              />
              <component :is="choice.icon" class="size-5" />
              {{ t(`backup.kinds.${choice.kind}`) }}
            </span>
            <span class="text-xs text-muted-foreground">{{
              t(`backup.kindHints.${choice.kind}`)
            }}</span>
          </label>
        </div>
        <p class="text-xs text-muted-foreground">{{ t('backup.later') }}</p>
      </div>
      <PlacesFields
        v-else-if="current === 'places'"
        v-model="form"
        :folder="folder"
        :task="task?.id"
        :password-set="task?.password_set"
      />
      <OptionsFields v-else-if="current === 'options'" v-model="form" />
      <dl v-else class="grid gap-2 text-sm" data-testid="backup-review">
        <h4 class="font-semibold">{{ t('backup.wizard.review') }}</h4>
        <div
          v-for="[label, value] in reviewRows"
          :key="label"
          class="grid grid-cols-[12rem_1fr] gap-2 border-b py-1.5"
        >
          <dt class="text-muted-foreground">{{ label }}</dt>
          <dd class="break-words">{{ value }}</dd>
        </div>
      </dl>
      <ul
        v-if="current !== 'kind' && current !== 'review' && found.length"
        class="grid gap-1 text-xs text-muted-foreground"
      >
        <li
          v-for="problem in found.filter((p) => editing || STEP_PROBLEMS[step]!.includes(p))"
          :key="problem"
          :data-problem="problem"
        >
          {{ t(`backup.wizard.problems.${problem}`) }}
        </li>
      </ul>
      <Alert v-if="error" variant="destructive">{{ error }}</Alert>
    </form>
    <template #footer>
      <Button v-if="!editing && step > 0" type="button" variant="secondary" @click="step--">{{
        t('backup.wizard.back')
      }}</Button>
      <Button v-if="!editing && step < 3" type="submit" form="backup-task" :disabled="!stepDone">{{
        t('backup.wizard.next')
      }}</Button>
      <Button v-else type="submit" form="backup-task" :disabled="pending || found.length > 0">{{
        editing ? t('backup.wizard.save') : t('backup.wizard.create')
      }}</Button>
    </template>
  </StepDialog>
</template>

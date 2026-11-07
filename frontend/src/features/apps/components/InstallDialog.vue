<script setup lang="ts">
import { Eye, EyeOff, Folder, FolderOpen } from 'lucide-vue-next'
import { computed, reactive, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { CatalogAppDetail, ComposeRisk } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { useJobsStore } from '@/shared/jobs/store'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { CodeEditor } from '@/shared/ui/code-editor'
import { ModalDialog } from '@/shared/ui/dialog'
import { Input } from '@/shared/ui/input'
import { Label } from '@/shared/ui/label'
import { RisksDialog } from '@/shared/ui/risks-dialog'

import {
  checkCompose,
  createProject,
  fillTemplate,
  useInstallActions,
  usePlaces,
  useShareList,
} from '../api'
import { appText, asksForMore, paragraphs } from '../texts'
import FolderPicker from './FolderPicker.vue'
import PermissionList from './PermissionList.vue'

const props = defineProps<{ app: CatalogAppDetail }>()
const open = defineModel<boolean>('open', { default: false })
const emit = defineEmits<{ installed: [project: string] }>()

/** A Compose project's name, as Docker Compose allows it. */
const NAME = /^[a-z0-9][a-z0-9_-]{0,62}$/

const { t, locale } = useI18n()
const actions = useInstallActions()
const jobs = useJobsStore()
const { data: places } = usePlaces()
const { data: shareList } = useShareList()

/** Where the project goes, what the app gets, then the file it becomes. */
const step = ref<'place' | 'values' | 'review'>('place')
const name = ref('')
/** The project's folder as `share/folder/…`. */
const location = ref('')
/** The folder follows the name until it is typed in or picked. */
const locationTouched = ref(false)
/** The base folder is being picked in the folder browser. */
const browsing = ref(false)

const ports = reactive<Record<string, string>>({})
const shares = reactive<Record<string, string>>({})
const settings = reactive<Record<string, string>>({})
/** The share whose folder is being picked. */
const picking = ref<string | null>(null)
/** Secrets shown in clear while typing. */
const revealed = reactive<Record<string, boolean>>({})
/** The app's Compose file filled in, as it will be written; it can still be changed. */
const compose = ref('')
const busy = ref(false)
const failure = ref<string | null>(null)
const risks = ref<ComposeRisk[]>([])
const confirming = ref(false)

const asked = computed(() => props.app.settings.filter((setting) => !setting.generate))
const generated = computed(() => props.app.settings.filter((setting) => setting.generate))
const permissions = computed(() => props.app.permissions)
const asksMore = computed(() => asksForMore(permissions.value))
const notes = computed(() => paragraphs(appText(props.app, 'notes', locale.value)))
/** The share and the folder in it of the location given. */
const parsed = computed(() => {
  const [share = '', ...rest] = trimmed(location.value).split('/')
  return { share, folder: rest.join('/') }
})
const shareId = computed(
  () => places.value?.find((place) => place.share === parsed.value.share)?.share_id ?? 0,
)
const shareName = computed(() => (shareId.value ? parsed.value.share : ''))
const folder = computed(() => parsed.value.folder)
const nameValid = computed(() => NAME.test(name.value))
const locationValid = computed(() => shareId.value > 0 && folder.value !== '')
const placed = computed(() => nameValid.value && locationValid.value)
const filledIn = computed(
  () =>
    props.app.shares.every((share) => shares[share.id]) &&
    asked.value.every((setting) => !setting.required || settings[setting.id]?.trim()),
)
const picked = computed(() => props.app.shares.find((share) => share.id === picking.value))

/** Fresh values whenever the dialog opens. */
watch(
  open,
  (isOpen) => {
    if (!isOpen) return
    name.value = props.app.id
    locationTouched.value = false
    location.value = proposed(props.app.id)
    for (const port of props.app.ports) ports[port.id] = String(port.default)
    for (const share of props.app.shares) shares[share.id] = ''
    for (const setting of asked.value) {
      settings[setting.id] = setting.default ?? (setting.kind === 'boolean' ? 'false' : '')
      revealed[setting.id] = false
    }
    compose.value = ''
    failure.value = null
    step.value = 'place'
  },
  { immediate: true },
)
// The places may come after the dialog opened; the name may change.
watch([places, name], () => {
  if (!locationTouched.value) location.value = proposed(name.value)
})

/** A path without slashes around it. */
function trimmed(path: string): string {
  return path.trim().replace(/^\/+|\/+$/g, '')
}

/**
 * Where a project is proposed: in a share called `docker` if there is one,
 * otherwise in a folder `docker` of the first share it may go in.
 */
function proposed(project: string): string {
  const list = places.value ?? []
  if (!project || !list.length) return ''
  if (list.some((place) => place.share === 'docker')) return `docker/${project}`
  return `${list[0]!.share}/docker/${project}`
}

/** A folder picked in the browser; a share by itself gets a folder named after the project. */
function pickBase(path: string): void {
  const picked = trimmed(path)
  location.value = picked.includes('/') ? picked : `${picked}/${name.value}`
  locationTouched.value = true
}

function inputType(kind: string, id: string): string {
  if (kind === 'password') return revealed[id] ? 'text' : 'password'
  return kind === 'number' ? 'number' : 'text'
}

function toggle(id: string, event: Event): void {
  settings[id] = (event.target as HTMLInputElement).checked ? 'true' : 'false'
}

function back(): void {
  failure.value = null
  step.value = step.value === 'review' ? 'values' : 'place'
}

/** The app's file filled in with what was given: shown before anything is made. */
async function fill(): Promise<void> {
  if (!filledIn.value || busy.value) return
  busy.value = true
  failure.value = null
  try {
    compose.value = await fillTemplate(props.app, {
      ports: Object.fromEntries(props.app.ports.map((port) => [port.id, Number(ports[port.id])])),
      shares: { ...shares },
      settings: Object.fromEntries(
        asked.value.map((setting) => [setting.id, settings[setting.id] ?? '']),
      ),
    })
    step.value = 'review'
  } catch (problem) {
    failure.value = problemMessage(problem)
  } finally {
    busy.value = false
  }
}

const place = () => ({ name: name.value, share_id: shareId.value, folder: folder.value })

/** Checks the file as Docker reads it; what reaches beyond the containers is confirmed first. */
async function deploy(): Promise<void> {
  if (busy.value || !compose.value.trim()) return
  busy.value = true
  failure.value = null
  try {
    risks.value = await checkCompose({ ...place(), compose: compose.value })
    if (risks.value.length) confirming.value = true
    else await make(false)
  } catch (problem) {
    failure.value = problemMessage(problem)
  } finally {
    busy.value = false
  }
}

async function make(accept: boolean): Promise<void> {
  busy.value = true
  failure.value = null
  try {
    const job = await createProject({
      ...place(),
      compose: compose.value,
      accept_risks: accept,
      template: { source_id: props.app.source_id, app: props.app.id },
    })
    if (job) jobs.update(job)
    await actions.refresh()
    open.value = false
    emit('installed', name.value)
  } catch (problem) {
    failure.value = problemMessage(problem)
  } finally {
    busy.value = false
  }
}

const titles = {
  place: 'appStore.placeTitle',
  values: 'appStore.configTitle',
  review: 'appStore.reviewTitle',
} as const
const filled =
  'h-10 w-full rounded-lg border-transparent bg-foreground/[0.05] px-3 text-sm shadow-none'
const label = 'gap-1 text-sm font-semibold'
const hint = 'text-xs leading-relaxed text-muted-foreground'
const required = 'text-destructive'
</script>

<!--
  An app of a catalog made into a Docker project of one's own: its place
  in a share, what it gets from the NAS, then the Compose file it becomes,
  which is written there and runs like any other project.
-->
<template>
  <ModalDialog
    v-model:open="open"
    :title="t(titles[step])"
    :description="
      step === 'place'
        ? undefined
        : t('appStore.configFor', {
            name: appText(app, 'name', locale),
            place: `${shareName}/${folder.trim()}`,
          })
    "
    :close-label="t('desktop.close')"
    :wide="step === 'review'"
  >
    <form
      v-if="step === 'place'"
      id="install-place"
      class="grid gap-5"
      @submit.prevent="step = 'values'"
    >
      <p :class="hint">{{ t('appStore.placeHint') }}</p>
      <div class="grid gap-2">
        <Label for="install-name" :class="label">{{ t('appStore.projectName') }}</Label>
        <Input
          id="install-name"
          v-model="name"
          :class="filled"
          class="font-mono"
          autocomplete="off"
          spellcheck="false"
          :aria-invalid="name !== '' && !nameValid"
        />
        <p :class="name !== '' && !nameValid ? 'text-xs text-destructive' : hint">
          {{ t('appStore.projectNameHint') }}
        </p>
      </div>
      <div class="grid gap-2">
        <Label for="install-folder" :class="label">{{ t('appStore.baseFolder') }}</Label>
        <div class="flex min-w-0 gap-2">
          <Input
            id="install-folder"
            v-model="location"
            :class="filled"
            class="min-w-0 flex-1 font-mono"
            autocomplete="off"
            spellcheck="false"
            :aria-invalid="location.trim() !== '' && !locationValid"
            @input="locationTouched = true"
          />
          <Button
            type="button"
            variant="outline"
            class="shrink-0"
            :disabled="!shareList?.length"
            @click="browsing = true"
          >
            <FolderOpen />
            {{ t('appStore.browse') }}
          </Button>
        </div>
        <p v-if="places && !places.length" class="text-xs text-destructive">
          {{ t('appStore.noPlace') }}
        </p>
        <p v-else-if="location.trim() !== '' && !locationValid" class="text-xs text-destructive">
          {{ t('appStore.notAPlace') }}
        </p>
        <p v-else :class="hint">{{ t('appStore.baseFolderHint') }}</p>
      </div>
    </form>

    <form
      v-else-if="step === 'values'"
      id="install-values"
      class="grid gap-5"
      @submit.prevent="fill"
    >
      <div v-for="share in app.shares" :key="`share-${share.id}`" class="grid gap-2">
        <Label :for="`share-${share.id}`" :class="label">
          <span :class="required" aria-hidden="true">*</span>{{ share.label }}
          <span class="font-normal text-muted-foreground">
            ({{ share.read_write ? t('appStore.readWrite') : t('appStore.readOnly') }})
          </span>
        </Label>
        <p v-if="share.description" :class="hint">{{ share.description }}</p>
        <button
          :id="`share-${share.id}`"
          type="button"
          :class="filled"
          class="flex items-center gap-2 text-left outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50"
          :disabled="!shareList?.length"
          @click="picking = share.id"
        >
          <span
            class="min-w-0 flex-1 truncate"
            :class="!shares[share.id] && 'text-muted-foreground'"
          >
            {{ shares[share.id] || t('appStore.pleaseChoose') }}
          </span>
          <Folder class="size-4 shrink-0 text-muted-foreground" />
        </button>
        <p v-if="shareList && !shareList.length" class="text-xs text-destructive">
          {{ t('appStore.noShares') }}
        </p>
      </div>

      <div v-for="port in app.ports" :key="`port-${port.id}`" class="grid gap-2">
        <Label :for="`port-${port.id}`" :class="label">
          <span :class="required" aria-hidden="true">*</span>{{ port.label }}
        </Label>
        <p :class="hint">
          {{ port.fixed ? t('appStore.fixedHint') : t('appStore.portHint') }}
        </p>
        <Input
          :id="`port-${port.id}`"
          v-model="ports[port.id]"
          :class="filled"
          type="number"
          min="1"
          max="65535"
          required
          :disabled="port.fixed"
        />
      </div>

      <div v-for="setting in asked" :key="`setting-${setting.id}`" class="grid gap-2">
        <label v-if="setting.kind === 'boolean'" class="flex items-center gap-2 text-sm">
          <input
            :id="`setting-${setting.id}`"
            type="checkbox"
            class="size-4 accent-primary"
            :checked="settings[setting.id] === 'true'"
            @change="toggle(setting.id, $event)"
          />
          {{ setting.label }}
        </label>
        <template v-else>
          <Label :for="`setting-${setting.id}`" :class="label">
            <span v-if="setting.required" :class="required" aria-hidden="true">*</span
            >{{ setting.label }}
            <span v-if="!setting.required" class="font-normal text-muted-foreground">
              ({{ t('appStore.optional') }})
            </span>
          </Label>
          <div class="relative">
            <Input
              :id="`setting-${setting.id}`"
              v-model="settings[setting.id]"
              :class="[filled, setting.kind === 'password' && 'pr-10']"
              :type="inputType(setting.kind, setting.id)"
              :required="setting.required"
              :placeholder="t('appStore.pleaseEnter')"
              autocomplete="off"
            />
            <button
              v-if="setting.kind === 'password'"
              type="button"
              class="absolute top-1/2 right-2 grid size-7 -translate-y-1/2 place-items-center rounded-md text-muted-foreground hover:text-foreground"
              :aria-label="revealed[setting.id] ? t('appStore.hideValue') : t('appStore.showValue')"
              @click="revealed[setting.id] = !revealed[setting.id]"
            >
              <EyeOff v-if="revealed[setting.id]" class="size-4" />
              <Eye v-else class="size-4" />
            </button>
          </div>
        </template>
        <p v-if="setting.description" :class="hint">{{ setting.description }}</p>
      </div>

      <p v-if="generated.length" :class="hint">
        {{
          t(
            'appStore.generated',
            { labels: generated.map((setting) => setting.label).join(', ') },
            generated.length,
          )
        }}
      </p>

      <section
        v-if="notes.length || asksMore || !app.official"
        class="grid gap-2 rounded-lg bg-amber-500/[0.07] p-4 text-sm"
        :aria-label="t('appStore.notice')"
      >
        <h3 class="font-medium">{{ t('appStore.notice') }}</h3>
        <div :class="hint" class="grid gap-2">
          <p v-if="!app.official">{{ t('appStore.unofficialHint') }}</p>
          <p v-for="(paragraph, index) in notes" :key="index">{{ paragraph }}</p>
          <template v-if="asksMore">
            <p class="font-medium text-foreground">{{ t('appStore.permissionsTitle') }}</p>
            <PermissionList :permissions="permissions" />
          </template>
        </div>
      </section>
    </form>

    <form v-else id="install-review" class="grid gap-3" @submit.prevent="deploy">
      <p :class="hint">
        {{ t('appStore.reviewHint', { file: `${shareName}/${folder.trim()}/compose.yaml` }) }}
      </p>
      <CodeEditor v-model="compose" :label="t('appStore.compose')" :min-rows="16" />
    </form>

    <Alert v-if="failure" variant="destructive" class="whitespace-pre-wrap">{{ failure }}</Alert>

    <FolderPicker v-model:open="browsing" :value="location" writable @pick="pickBase" />
    <FolderPicker
      :open="picking !== null"
      :value="picking ? (shares[picking] ?? '') : ''"
      :writable="picked?.read_write ?? false"
      @update:open="(value: boolean) => !value && (picking = null)"
      @pick="(path: string) => picking && (shares[picking] = path)"
    />
    <RisksDialog v-model:open="confirming" :risks="risks" @confirm="make(true)" />

    <template #footer>
      <template v-if="step === 'place'">
        <Button type="button" variant="secondary" @click="open = false">{{
          t('desktop.cancel')
        }}</Button>
        <Button type="submit" form="install-place" :disabled="!placed">{{
          t('appStore.next')
        }}</Button>
      </template>
      <template v-else>
        <Button type="button" variant="secondary" :disabled="busy" @click="back">{{
          t('desktop.back')
        }}</Button>
        <Button
          v-if="step === 'values'"
          type="submit"
          form="install-values"
          :disabled="!filledIn || busy"
          >{{ t('appStore.next') }}</Button
        >
        <Button v-else type="submit" form="install-review" :disabled="busy || !compose.trim()">{{
          t('appStore.install')
        }}</Button>
      </template>
    </template>
  </ModalDialog>
</template>

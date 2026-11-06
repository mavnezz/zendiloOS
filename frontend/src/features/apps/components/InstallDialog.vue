<script setup lang="ts">
import { ChevronDown, Eye, EyeOff, Folder } from 'lucide-vue-next'
import { computed, reactive, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { CatalogAppDetail } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { ModalDialog } from '@/shared/ui/dialog'
import { Input } from '@/shared/ui/input'
import { Label } from '@/shared/ui/label'

import { useDocker, useInstallActions, useShareList } from '../api'
import { appText, asksForMore, paragraphs } from '../texts'
import FolderPicker from './FolderPicker.vue'
import PermissionList from './PermissionList.vue'

const props = defineProps<{ app: CatalogAppDetail }>()
const open = defineModel<boolean>('open', { default: false })
const emit = defineEmits<{ installed: [id: string] }>()

const { t, locale } = useI18n()
const actions = useInstallActions()
const { data: shareList } = useShareList()
const { data: docker } = useDocker()

/** Where this browser remembers the volume apps always go on. */
const REMEMBERED = 'zendiloos.apps.pool'
function remembered(): string | null {
  try {
    return window.localStorage.getItem(REMEMBERED)
  } catch {
    return null
  }
}
function remember(pool: string): void {
  try {
    window.localStorage.setItem(REMEMBERED, pool)
  } catch {
    // Without storage, the question comes again next time.
  }
}

/** First where the app goes, then what it gets. */
const step = ref<'place' | 'values'>('place')
const pool = ref('')
const always = ref(false)
const volumes = computed(() => docker.value?.volumes ?? [])
const place = computed(() => volumes.value.find((volume) => volume.pool === pool.value))

const ports = reactive<Record<string, string>>({})
const shares = reactive<Record<string, string>>({})
const settings = reactive<Record<string, string>>({})
/** The share whose folder is being picked. */
const picking = ref<string | null>(null)
/** Secrets shown in clear while typing. */
const revealed = reactive<Record<string, boolean>>({})
const understood = ref(false)
const busy = ref(false)
const failure = ref<string | null>(null)

const asked = computed(() => props.app.settings.filter((setting) => !setting.generate))
const generated = computed(() => props.app.settings.filter((setting) => setting.generate))
const permissions = computed(() => props.app.permissions)
const asksMore = computed(() => asksForMore(permissions.value))
const notes = computed(() => paragraphs(appText(props.app, 'notes', locale.value)))
/** What reaches beyond the rules is installed only once read. */
const risky = computed(() => asksMore.value || !props.app.official)
const ready = computed(
  () =>
    !busy.value &&
    (!risky.value || understood.value) &&
    props.app.shares.every((share) => shares[share.id]) &&
    asked.value.every((setting) => !setting.required || settings[setting.id]?.trim()),
)

/** Fresh values whenever the dialog opens. */
watch(
  open,
  (isOpen) => {
    if (!isOpen) return
    for (const port of props.app.ports) ports[port.id] = String(port.default)
    for (const share of props.app.shares) shares[share.id] = ''
    for (const setting of asked.value) {
      settings[setting.id] = setting.default ?? (setting.kind === 'boolean' ? 'false' : '')
      revealed[setting.id] = false
    }
    understood.value = false
    failure.value = null
    const kept = remembered()
    const known = volumes.value.some((volume) => volume.pool === kept)
    pool.value = known && kept ? kept : (docker.value?.volume?.pool ?? volumes.value[0]?.pool ?? '')
    always.value = false
    step.value = known ? 'values' : 'place'
  },
  { immediate: true },
)

function next(): void {
  if (always.value) remember(pool.value)
  step.value = 'values'
}

function inputType(kind: string, id: string): string {
  if (kind === 'password') return revealed[id] ? 'text' : 'password'
  return kind === 'number' ? 'number' : 'text'
}

function toggle(id: string, event: Event): void {
  settings[id] = (event.target as HTMLInputElement).checked ? 'true' : 'false'
}

async function install(): Promise<void> {
  if (!ready.value) return
  busy.value = true
  failure.value = null
  try {
    await actions.install({
      source: props.app.source_id,
      app: props.app.id,
      ports: Object.fromEntries(props.app.ports.map((port) => [port.id, Number(ports[port.id])])),
      shares: { ...shares },
      settings: Object.fromEntries(
        asked.value.map((setting) => [setting.id, settings[setting.id] ?? '']),
      ),
      confirm_permissions: asksMore.value && understood.value,
      pool: pool.value || null,
    })
    open.value = false
    emit('installed', props.app.id)
  } catch (problem) {
    failure.value = problemMessage(problem)
  } finally {
    busy.value = false
  }
}

const filled =
  'h-10 w-full rounded-lg border-transparent bg-foreground/[0.05] px-3 text-sm shadow-none'
const select = `${filled} appearance-none pr-9 outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50`
const label = 'gap-1 text-sm font-semibold'
const hint = 'text-xs leading-relaxed text-muted-foreground'
const required = 'text-destructive'
</script>

<!--
  Asked once before an app is installed: first the volume it goes on, then
  what it gets from the NAS, with what to know before.
-->
<template>
  <ModalDialog
    v-model:open="open"
    :title="step === 'place' ? t('appStore.placeTitle') : t('appStore.configTitle')"
    :description="
      step === 'place'
        ? undefined
        : t('appStore.configFor', {
            name: appText(app, 'name', locale),
            place: place
              ? t('appStore.placeVolume', { number: place.number, pool: place.pool })
              : pool,
          })
    "
    :close-label="t('desktop.close')"
  >
    <form v-if="step === 'place'" id="install-place" class="grid gap-3" @submit.prevent="next">
      <Label for="install-pool" class="leading-normal font-normal">{{
        t('appStore.placeQuestion')
      }}</Label>
      <div class="relative">
        <select id="install-pool" v-model="pool" :class="select">
          <option v-for="volume in volumes" :key="volume.pool" :value="volume.pool">
            {{ t('appStore.placeVolume', { number: volume.number, pool: volume.pool }) }}
          </option>
        </select>
        <ChevronDown
          class="pointer-events-none absolute top-1/2 right-3 size-4 -translate-y-1/2 text-muted-foreground"
        />
      </div>
      <p :class="hint" class="pb-6">{{ t('appStore.placeHint') }}</p>
    </form>

    <form v-else id="install-values" class="grid gap-5" @submit.prevent="install">
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
        v-if="notes.length || risky"
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
      <label v-if="risky" class="flex items-start gap-2 text-sm">
        <input v-model="understood" type="checkbox" class="mt-0.5 size-4 accent-primary" />
        <span>{{ t('appStore.understood') }}</span>
      </label>

      <Alert v-if="failure" variant="destructive">{{ failure }}</Alert>
    </form>

    <FolderPicker
      :open="picking !== null"
      :value="picking ? (shares[picking] ?? '') : ''"
      @update:open="(value: boolean) => !value && (picking = null)"
      @pick="(path: string) => picking && (shares[picking] = path)"
    />

    <template #footer>
      <template v-if="step === 'place'">
        <label class="mr-auto flex min-w-0 flex-1 items-center gap-2 text-sm">
          <input v-model="always" type="checkbox" class="size-4 shrink-0 accent-primary" />
          {{ t('appStore.placeAlways') }}
        </label>
        <Button type="button" variant="secondary" @click="open = false">{{
          t('desktop.cancel')
        }}</Button>
        <Button type="submit" form="install-place" :disabled="!pool">{{
          t('appStore.next')
        }}</Button>
      </template>
      <template v-else>
        <Button type="button" variant="secondary" @click="step = 'place'">{{
          t('desktop.back')
        }}</Button>
        <Button type="submit" form="install-values" :disabled="!ready">{{
          t('appStore.install')
        }}</Button>
      </template>
    </template>
  </ModalDialog>
</template>

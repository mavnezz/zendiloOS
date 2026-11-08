<script setup lang="ts">
import { Plus, Trash2, TriangleAlert } from 'lucide-vue-next'
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type {
  Disk,
  EnclosureModel,
  EnclosureNumbering,
  EnclosureOrientation,
  SlotBinding,
  SlotForm,
} from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { ModalDialog } from '@/shared/ui/dialog'
import { Input } from '@/shared/ui/input'
import { NativeSelect } from '@/shared/ui/native-select'

import {
  resetEnclosure,
  saveEnclosure,
  useDisks,
  useEnclosure,
  useEnclosureModels,
  useRefreshStorage,
} from '../api'
import {
  bindingFor,
  bindSlot,
  boundDisk,
  type Draft,
  type DraftGroup,
  draftOf,
  groupsOf,
  idsOf,
  MAX_GROUPS,
  modelBinding,
  newGroup,
  overrideSlot,
  problemsOf,
  requestOf,
  resize,
  sameBinding,
} from '../lib/enclosure'
import BindingSelect from './BindingSelect.vue'

const open = defineModel<boolean>('open', { default: false })

const { t } = useI18n()
const { data: enclosure } = useEnclosure()
const { data: catalog } = useEnclosureModels()
const { data: disks } = useDisks()
const refresh = useRefreshStorage()

const FORMS: SlotForm[] = ['inch_3_5', 'inch_2_5', 'm2']
const ORIENTATIONS: EnclosureOrientation[] = ['vertical', 'horizontal']
const NUMBERINGS: EnclosureNumbering[] = [
  'left_to_right',
  'right_to_left',
  'top_to_bottom',
  'bottom_to_top',
]
const CUSTOM = 'custom'

const draft = ref<Draft | null>(null)
const failure = ref<string | null>(null)
const saving = ref(false)
/** Whether the draft was filled from the server since the dialog opened. */
let filled = false

// Opening starts from what is chosen, once loaded; a later refetch does not
// overwrite what is being edited.
watch(
  [open, () => enclosure.value, () => catalog.value],
  ([isOpen, current, models]) => {
    if (!isOpen) {
      filled = false
      return
    }
    if (filled || !current || !models) return
    filled = true
    draft.value = draftOf(current, models)
    failure.value = null
  },
  { immediate: true },
)

/** The detected model first, then the others of the catalog. */
const models = computed<EnclosureModel[]>(() => {
  const detected = enclosure.value?.detected?.id
  return [...(catalog.value ?? [])].sort(
    (a, b) => Number(b.id === detected) - Number(a.id === detected),
  )
})

const model = computed(() =>
  draft.value?.source === 'model'
    ? models.value.find((candidate) => candidate.id === (draft.value as { model: string }).model)
    : undefined,
)

function modelName(candidate: EnclosureModel): string {
  const name = `${candidate.vendor} ${candidate.name}`
  return candidate.id === enclosure.value?.detected?.id
    ? t('storage.enclosure.dialog.detectedTag', { model: name })
    : name
}

const source = computed({
  get: () => (draft.value?.source === 'model' ? draft.value.model : CUSTOM),
  set: (value: string) => {
    if (value === CUSTOM) {
      if (draft.value?.source !== 'custom') {
        draft.value = { source: 'custom', groups: [newGroup([])] }
      }
    } else {
      draft.value = { source: 'model', model: value, overrides: {} }
    }
  },
})

/** A model's slots as a layout of one's own, to change. */
function fromModel(): void {
  if (model.value) draft.value = { source: 'custom', groups: groupsOf(model.value) }
}

/** Every slot of the draft with what it holds now. */
const slots = computed(() => {
  const current = draft.value
  if (!current) return []
  if (current.source === 'model') {
    return (model.value?.groups ?? []).flatMap((group) =>
      group.slots.map((slot) => ({
        id: slot.id,
        label: slot.label,
        group: group.name,
        binding:
          slot.id in current.overrides
            ? (current.overrides[slot.id] ?? null)
            : (slot.binding ?? null),
      })),
    )
  }
  return current.groups.flatMap((group) =>
    group.slots.map((slot) => ({
      id: slot.id,
      label: slot.label,
      group: group.name,
      binding: slot.binding,
    })),
  )
})

function encode(binding: SlotBinding | null): string {
  return binding ? `${binding.kind}:${binding.value}` : 'none'
}

function decode(value: string): SlotBinding | null | 'model' {
  if (value === 'model') return 'model'
  const [kind, ...rest] = value.split(':')
  if (kind === 'port' || kind === 'serial') return { kind, value: rest.join(':') }
  return null
}

function diskLabel(disk: Disk): string {
  return t('storage.enclosure.dialog.disk', {
    disk: disk.name,
    model: disk.model ?? t('storage.unknownModel'),
  })
}

function describe(binding: SlotBinding): string {
  return t(`storage.enclosure.dialog.${binding.kind}`, { value: binding.value })
}

/** What the slot's list shows chosen: the matching disk's entry, or the binding itself. */
function selected(slot: { id: string; binding: SlotBinding | null }): string {
  const current = draft.value
  if (current?.source === 'model' && !(slot.id in current.overrides)) return 'model'
  const binding = slot.binding
  if (!binding) return 'none'
  const disk = boundDisk(binding, disks.value ?? [])
  if (!disk) return encode(binding)
  return binding.kind === 'port'
    ? encode({ kind: 'port', value: disk.id_path ?? binding.value })
    : encode(binding)
}

/** A binding no disk present has, still offered so it is not lost by looking. */
function absent(binding: SlotBinding | null): SlotBinding | null {
  return binding && !boundDisk(binding, disks.value ?? []) ? binding : null
}

/** The entry for a binding no disk present has. */
function absentChoice(binding: SlotBinding | null): { value: string; label: string } | null {
  const missing = absent(binding)
  return missing
    ? {
        value: encode(missing),
        label: t('storage.enclosure.dialog.absent', { binding: describe(missing) }),
      }
    : null
}

/** The entry that keeps what the model binds at `slot`. */
function modelChoice(slot: string): string {
  const binding = model.value ? modelBinding(model.value, slot) : null
  return t('storage.enclosure.dialog.asModel', {
    binding: binding ? describe(binding) : t('storage.enclosure.dialog.nothing'),
  })
}

function bind(slot: string, value: string): void {
  const current = draft.value
  const binding = decode(value)
  if (!current) return
  if (current.source === 'model') {
    if (model.value) current.overrides = overrideSlot(model.value, current.overrides, slot, binding)
  } else {
    current.groups = bindSlot(current.groups, slot, binding === 'model' ? null : binding)
  }
}

/** Present disks the draft puts in no slot. */
const unplaced = computed(() =>
  (disks.value ?? []).filter(
    (disk) => !slots.value.some((slot) => boundDisk(slot.binding, [disk]) === disk),
  ),
)

function place(disk: Disk, slot: string): void {
  const binding = bindingFor(disk)
  if (slot && binding) bind(slot, encode(binding))
}

function groups(): DraftGroup[] {
  return draft.value?.source === 'custom' ? draft.value.groups : []
}

function addGroup(): void {
  if (draft.value?.source === 'custom' && draft.value.groups.length < MAX_GROUPS) {
    draft.value.groups = [
      ...draft.value.groups,
      newGroup(
        draft.value.groups,
        `${t('storage.enclosure.dialog.group', { n: draft.value.groups.length + 1 })}`,
      ),
    ]
  }
}

function removeGroup(index: number): void {
  if (draft.value?.source === 'custom') {
    draft.value.groups = draft.value.groups.filter((_, at) => at !== index)
  }
}

function resized(index: number, rows: number, columns: number): void {
  if (draft.value?.source !== 'custom') return
  const others = draft.value.groups.filter((_, at) => at !== index)
  const group = draft.value.groups[index]
  if (!group) return
  const next = resize(group, rows, columns, new Set(idsOf(others)))
  draft.value.groups = draft.value.groups.map((candidate, at) => (at === index ? next : candidate))
}

const problems = computed(() =>
  draft.value?.source === 'custom' ? problemsOf(draft.value.groups) : [],
)

async function save(): Promise<void> {
  if (!draft.value || problems.value.length) return
  failure.value = null
  saving.value = true
  try {
    await saveEnclosure(requestOf(draft.value))
    await refresh()
    open.value = false
  } catch (problem) {
    failure.value = problemMessage(problem)
  } finally {
    saving.value = false
  }
}

async function remove(): Promise<void> {
  failure.value = null
  saving.value = true
  try {
    await resetEnclosure()
    await refresh()
    open.value = false
  } catch (problem) {
    failure.value = problemMessage(problem)
  } finally {
    saving.value = false
  }
}
</script>

<!-- Which enclosure the NAS is, its slots, and which disk sits in which. -->
<template>
  <ModalDialog
    v-model:open="open"
    wide
    :title="t('storage.enclosure.dialog.title')"
    :description="t('storage.enclosure.dialog.description')"
    :close-label="t('desktop.close')"
  >
    <Alert v-if="failure" variant="destructive">{{ failure }}</Alert>
    <form v-if="draft" id="enclosure-form" class="grid gap-5" @submit.prevent="save">
      <div class="grid gap-1.5">
        <label for="enclosure-source" class="text-sm font-medium">
          {{ t('storage.enclosure.dialog.source') }}
        </label>
        <NativeSelect id="enclosure-source" v-model="source">
          <option v-for="candidate in models" :key="candidate.id" :value="candidate.id">
            {{ modelName(candidate) }}
          </option>
          <option :value="CUSTOM">{{ t('storage.enclosure.dialog.custom') }}</option>
        </NativeSelect>
      </div>

      <template v-if="draft.source === 'model' && model">
        <Alert v-if="!model.verified">
          <TriangleAlert class="text-amber-600 dark:text-amber-400" />
          <div class="grid gap-1">
            <p>{{ t('storage.enclosure.dialog.unverified') }}</p>
            <p v-if="model.notes" class="text-xs text-muted-foreground">{{ model.notes }}</p>
          </div>
        </Alert>
        <div class="flex justify-end">
          <Button type="button" variant="outline" size="sm" @click="fromModel">
            {{ t('storage.enclosure.dialog.fromModel') }}
          </Button>
        </div>
      </template>

      <section v-if="draft.source === 'custom'" class="grid gap-3">
        <div class="flex items-center gap-2">
          <h4 class="flex-1 text-sm font-semibold">{{ t('storage.enclosure.dialog.groups') }}</h4>
          <Button
            type="button"
            variant="outline"
            size="sm"
            :disabled="draft.groups.length >= MAX_GROUPS"
            @click="addGroup"
          >
            <Plus />
            {{ t('storage.enclosure.dialog.addGroup') }}
          </Button>
        </div>
        <fieldset
          v-for="(group, index) in groups()"
          :key="group.id"
          class="grid gap-3 rounded-lg border p-3"
        >
          <legend class="sr-only">{{ group.name }}</legend>
          <div
            class="grid grid-cols-2 gap-2 @2xl:grid-cols-[2fr_repeat(2,5rem)_repeat(3,1fr)_auto]"
          >
            <label class="col-span-2 grid gap-1 text-xs @2xl:col-span-1">
              {{ t('storage.enclosure.dialog.name') }}
              <Input v-model="group.name" :maxlength="64" />
            </label>
            <label class="grid gap-1 text-xs">
              {{ t('storage.enclosure.dialog.rows') }}
              <input
                type="number"
                min="1"
                max="60"
                :value="group.rows"
                class="h-8 rounded-md border bg-background px-2 text-sm tabular-nums"
                @change="
                  resized(index, Number(($event.target as HTMLInputElement).value), group.columns)
                "
              />
            </label>
            <label class="grid gap-1 text-xs">
              {{ t('storage.enclosure.dialog.columns') }}
              <input
                type="number"
                min="1"
                max="60"
                :value="group.columns"
                class="h-8 rounded-md border bg-background px-2 text-sm tabular-nums"
                @change="
                  resized(index, group.rows, Number(($event.target as HTMLInputElement).value))
                "
              />
            </label>
            <label class="grid gap-1 text-xs">
              {{ t('storage.enclosure.dialog.form') }}
              <NativeSelect v-model="group.form">
                <option v-for="form in FORMS" :key="form" :value="form">
                  {{ t(`storage.enclosure.dialog.forms.${form}`) }}
                </option>
              </NativeSelect>
            </label>
            <label class="grid gap-1 text-xs">
              {{ t('storage.enclosure.dialog.orientation') }}
              <NativeSelect v-model="group.orientation">
                <option v-for="orientation in ORIENTATIONS" :key="orientation" :value="orientation">
                  {{ t(`storage.enclosure.dialog.orientations.${orientation}`) }}
                </option>
              </NativeSelect>
            </label>
            <label class="grid gap-1 text-xs">
              {{ t('storage.enclosure.dialog.numbering') }}
              <NativeSelect v-model="group.numbering">
                <option v-for="numbering in NUMBERINGS" :key="numbering" :value="numbering">
                  {{ t(`storage.enclosure.dialog.numberings.${numbering}`) }}
                </option>
              </NativeSelect>
            </label>
            <Button
              type="button"
              variant="ghost"
              size="icon"
              class="self-end"
              :aria-label="t('storage.enclosure.dialog.removeGroup')"
              @click="removeGroup(index)"
            >
              <Trash2 />
            </Button>
          </div>
          <ol class="grid gap-1.5 @xl:grid-cols-2">
            <li
              v-for="(slot, position) in group.slots"
              :key="slot.id"
              class="flex items-center gap-2"
            >
              <Input
                v-model="slot.label"
                :maxlength="32"
                class="w-28 shrink-0"
                :aria-label="t('storage.enclosure.dialog.label', { n: position + 1 })"
              />
              <BindingSelect
                class="flex-1"
                :label="slot.label"
                :value="selected(slot)"
                :disks="disks ?? []"
                :absent="absentChoice(slot.binding)"
                @change="bind(slot.id, $event)"
              />
            </li>
          </ol>
        </fieldset>
        <ul v-if="problems.length" class="grid gap-1 text-sm text-destructive">
          <li v-for="problem in problems" :key="problem">
            {{ t(`storage.enclosure.dialog.problems.${problem}`) }}
          </li>
        </ul>
      </section>

      <section v-if="draft.source === 'model' && model" class="grid gap-2">
        <h4 class="text-sm font-semibold">{{ t('storage.enclosure.dialog.slots') }}</h4>
        <div class="overflow-x-auto rounded-lg border">
          <table class="w-full text-sm">
            <thead class="bg-muted/50 text-left text-xs text-muted-foreground">
              <tr>
                <th class="px-3 py-2 font-medium">{{ t('storage.enclosure.dialog.slot') }}</th>
                <th class="px-3 py-2 font-medium">{{ t('storage.enclosure.dialog.holds') }}</th>
              </tr>
            </thead>
            <tbody>
              <tr v-for="slot in slots" :key="slot.id" class="border-t">
                <td class="px-3 py-1.5 whitespace-nowrap">
                  <span class="font-medium">{{ slot.label }}</span>
                  <span class="block text-xs text-muted-foreground">{{ slot.group }}</span>
                </td>
                <td class="px-3 py-1.5">
                  <BindingSelect
                    class="w-full"
                    :label="slot.label"
                    :value="selected(slot)"
                    :disks="disks ?? []"
                    :model-choice="modelChoice(slot.id)"
                    :absent="
                      sameBinding(slot.binding, modelBinding(model, slot.id))
                        ? null
                        : absentChoice(slot.binding)
                    "
                    @change="bind(slot.id, $event)"
                  />
                </td>
              </tr>
            </tbody>
          </table>
        </div>
      </section>

      <section v-if="unplaced.length" class="grid gap-2">
        <h4 class="text-sm font-semibold">{{ t('storage.enclosure.dialog.assign') }}</h4>
        <p class="text-xs text-muted-foreground">{{ t('storage.enclosure.dialog.assignHint') }}</p>
        <ul class="grid gap-1.5">
          <li
            v-for="disk in unplaced"
            :key="disk.name"
            class="flex flex-wrap items-center gap-2 text-sm"
          >
            <span class="min-w-0 flex-1 truncate">{{ diskLabel(disk) }}</span>
            <select
              value=""
              :disabled="!bindingFor(disk)"
              :aria-label="t('storage.enclosure.dialog.putInto', { disk: disk.name })"
              class="h-8 w-56 rounded-md border bg-background px-2 text-sm outline-none focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50 disabled:opacity-50 dark:bg-input/30"
              @change="place(disk, ($event.target as HTMLSelectElement).value)"
            >
              <option value="">{{ t('storage.enclosure.dialog.noSlot') }}</option>
              <option v-for="slot in slots" :key="slot.id" :value="slot.id">
                {{ t('storage.enclosure.slot', { slot: slot.label, group: slot.group }) }}
              </option>
            </select>
          </li>
        </ul>
      </section>
    </form>
    <template #footer>
      <Button
        v-if="enclosure?.source"
        type="button"
        variant="ghost"
        class="mr-auto text-destructive"
        :disabled="saving"
        @click="remove"
      >
        {{ t('storage.enclosure.dialog.remove') }}
      </Button>
      <Button type="button" variant="outline" @click="open = false">
        {{ t('desktop.cancel') }}
      </Button>
      <Button
        type="submit"
        form="enclosure-form"
        :disabled="saving || !draft || problems.length > 0"
      >
        {{ t('storage.enclosure.dialog.save') }}
      </Button>
    </template>
  </ModalDialog>
</template>

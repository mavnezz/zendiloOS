<script setup lang="ts">
import { useQuery } from '@tanstack/vue-query'
import { Copy, FolderSync, Settings2 } from 'lucide-vue-next'
import { computed, ref, shallowRef, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import { api, unwrap } from '@/shared/api/client'
import type { FileServices } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { useNotifications } from '@/shared/lib/notifications'
import { Button } from '@/shared/ui/button'
import { Input } from '@/shared/ui/input'
import { NativeSelect } from '@/shared/ui/native-select'
import {
  ComingLater,
  PageDescription,
  PageFooter,
  PageHeader,
  PageTabs,
  SettingRow,
} from '@/shared/ui/page'
import { Switch } from '@/shared/ui/switch'

import { useFileServices, useSetFileServices } from '../../sharing'
import { fromDraft, sameServices, type ServicesDraft, toDraft } from '../../lib/services'
import SmbOptionsDialog from './SmbOptionsDialog.vue'

/** As the server checks it. */
const WORKGROUP = /^[A-Za-z0-9_][A-Za-z0-9_-]{0,14}$/

/** Where clients reach the NAS: its name, and its address if known. */
const props = defineProps<{ canManage: boolean; hostname?: string; address?: string }>()

const { t } = useI18n()
const notifications = useNotifications()
const { data, isPending } = useFileServices()
const setServices = useSetFileServices()
const { data: shares } = useQuery({
  queryKey: ['shares'],
  queryFn: () => unwrap(api.GET('/api/v1/shares')),
})

const tab = ref('smb')
const advanced = ref(false)
const draft = ref<ServicesDraft>()
/** The settings the draft started from. */
const base = shallowRef<ServicesDraft>()

function reset(services: FileServices | undefined = data.value): void {
  if (!services) return
  base.value = toDraft(services)
  draft.value = toDraft(services)
  setServices.reset()
}
const dirty = computed(() =>
  Boolean(draft.value && base.value && !sameServices(draft.value, base.value)),
)
watch(
  data,
  (services) => {
    if (!base.value || !dirty.value) reset(services)
  },
  { immediate: true },
)

const workgroupValid = computed(() => WORKGROUP.test(draft.value?.workgroup.trim() ?? ''))
const timeMachineValid = computed(() => {
  const value = draft.value
  if (!value?.time_machine_share) return true
  const gib = Number(value.time_machine_gib)
  return value.mac_compat && (value.time_machine_gib === '' || gib >= 1)
})

function apply(services: ServicesDraft | undefined = draft.value): void {
  if (!services) return
  setServices.mutate(fromDraft(services), {
    onSuccess: (stored) => {
      reset(stored)
      advanced.value = false
    },
  })
}

const hostname = computed(() => props.hostname ?? 'nas')
const address = computed(() => props.address)
const connect = computed(() => [
  {
    system: 'Windows',
    paths: [`\\\\${hostname.value}`, ...(address.value ? [`\\\\${address.value}`] : [])],
  },
  {
    system: 'macOS',
    paths: [`smb://${hostname.value}.local`, ...(address.value ? [`smb://${address.value}`] : [])],
  },
  { system: 'Linux', paths: [`smb://${address.value ?? hostname.value}/`] },
])
async function copy(text: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(text)
    notifications.notify('success', t('fileServices.copied', { text }))
  } catch {
    notifications.notify('error', t('fileServices.copyFailed'))
  }
}

const tabs = computed(() => [
  { id: 'smb', label: 'SMB' },
  { id: 'nfs', label: 'NFS' },
  { id: 'rsync', label: 'rsync' },
  { id: 'advanced', label: t('fileServices.tabs.advanced') },
])
const soon = { rsync: FolderSync } as const
const isSoon = (id: string): id is keyof typeof soon => id in soon
</script>

<!--
  The protocols that serve the shares, one tab each, as on the appliances
  people know. What most never change about SMB is behind "Advanced".
-->
<template>
  <div class="flex min-h-full flex-col">
    <PageHeader :title="t('fileServices.title')" :description="t('fileServices.description')" />
    <PageTabs v-model="tab" :label="t('fileServices.title')" :tabs="tabs" />

    <div v-if="isPending || !draft" class="h-64 max-w-2xl animate-pulse rounded-lg bg-muted" />

    <template v-else-if="tab === 'smb'">
      <PageDescription>
        <p>{{ t('fileServices.explanation') }}</p>
        <p>{{ t('fileServices.smb1Note') }}</p>
      </PageDescription>
      <div class="grid max-w-3xl gap-5">
        <SettingRow :label="t('fileServices.smbSettings')" for="service-smb">
          <label class="flex items-center gap-3 pt-1 text-sm">
            <Switch id="service-smb" v-model="draft.smb" :disabled="!canManage" />
            {{ t('fileServices.smbOn') }}
          </label>
        </SettingRow>
        <SettingRow :label="t('fileServices.workgroup')" for="smb-workgroup">
          <Input
            id="smb-workgroup"
            v-model="draft.workgroup"
            :disabled="!canManage || !draft.smb"
            :aria-invalid="!workgroupValid"
            maxlength="15"
            autocomplete="off"
            spellcheck="false"
            class="max-w-xs font-mono text-dense uppercase"
          />
          <p :class="workgroupValid ? 'text-xs text-muted-foreground' : 'text-xs text-destructive'">
            {{
              workgroupValid
                ? t('fileServices.workgroupDescription')
                : t('fileServices.workgroupInvalid')
            }}
          </p>
        </SettingRow>
        <SettingRow label="">
          <Button
            variant="secondary"
            class="w-fit"
            :disabled="!canManage || !draft.smb"
            @click="advanced = true"
          >
            <Settings2 />
            {{ t('fileServices.tabs.advanced') }}
          </Button>
        </SettingRow>
      </div>

      <section class="mt-8 max-w-3xl border-t pt-5">
        <h3 class="mb-2 text-sm font-medium">{{ t('fileServices.connect') }}</h3>
        <ul class="grid gap-2 text-sm">
          <li
            v-for="client in connect"
            :key="client.system"
            class="flex flex-wrap items-center gap-x-2 gap-y-1"
          >
            <span class="w-20 text-muted-foreground">{{ client.system }}</span>
            <template v-for="(path, index) in client.paths" :key="path">
              <span v-if="index > 0" class="text-muted-foreground">{{ t('fileServices.or') }}</span>
              <button
                type="button"
                class="group inline-flex items-center gap-1.5 rounded-md bg-muted px-2 py-0.5 font-mono text-dense hover:bg-accent"
                :title="t('fileServices.copy')"
                @click="copy(path)"
              >
                {{ path }}
                <Copy class="size-3.5 text-muted-foreground group-hover:text-foreground" />
              </button>
            </template>
          </li>
        </ul>
        <p class="mt-3 text-sm text-muted-foreground">
          {{ t('fileServices.timeMachineHint') }}
          <button type="button" class="text-primary hover:underline" @click="tab = 'advanced'">
            {{ t('fileServices.tabs.advanced') }}
          </button>
        </p>
      </section>
    </template>

    <template v-else-if="tab === 'nfs'">
      <PageDescription>
        <p>{{ t('fileServices.nfsExplanation') }}</p>
      </PageDescription>
      <div class="grid max-w-3xl gap-5">
        <SettingRow :label="t('fileServices.nfsSettings')" for="service-nfs">
          <label class="flex items-center gap-3 pt-1 text-sm">
            <Switch id="service-nfs" v-model="draft.nfs" :disabled="!canManage" />
            {{ t('fileServices.nfsOn') }}
          </label>
          <p class="text-xs text-muted-foreground">{{ t('fileServices.nfsHint') }}</p>
        </SettingRow>
      </div>
    </template>

    <template v-else-if="tab === 'advanced'">
      <PageDescription>
        <p>{{ t('fileServices.discoveryExplanation') }}</p>
      </PageDescription>
      <div class="grid max-w-3xl gap-6">
        <SettingRow :label="t('fileServices.discovery')" for="service-wsdd">
          <label class="flex items-center gap-3 pt-1 text-sm">
            <Switch id="service-wsdd" v-model="draft.wsdd" :disabled="!canManage || !draft.smb" />
            {{ t('fileServices.wsddOn') }}
          </label>
          <p class="text-xs text-muted-foreground">{{ t('fileServices.wsddHint') }}</p>
          <label class="mt-2 flex items-center gap-3 text-sm">
            <Switch id="service-bonjour" v-model="draft.bonjour" :disabled="!canManage" />
            {{ t('fileServices.bonjourOn') }}
          </label>
          <p class="text-xs text-muted-foreground">{{ t('fileServices.bonjourHint') }}</p>
        </SettingRow>
        <SettingRow label="Time Machine" for="time-machine-share">
          <div class="flex flex-wrap items-center gap-2">
            <NativeSelect
              id="time-machine-share"
              v-model.number="draft.time_machine_share"
              class="w-56"
              :disabled="!canManage || !draft.mac_compat"
            >
              <option :value="0">{{ t('fileServices.noTimeMachine') }}</option>
              <option
                v-for="share in shares?.filter(
                  (s) => s.shared || s.id === draft?.time_machine_share,
                )"
                :key="share.id"
                :value="share.id"
              >
                {{ share.name }}
              </option>
            </NativeSelect>
            <template v-if="draft.time_machine_share">
              <Input
                v-model="draft.time_machine_gib"
                type="number"
                min="1"
                step="any"
                class="w-28"
                :placeholder="t('people.unlimited')"
                :aria-label="t('fileServices.timeMachineSize')"
              />
              <span class="text-sm text-muted-foreground">GiB</span>
            </template>
          </div>
          <p
            class="text-xs"
            :class="timeMachineValid ? 'text-muted-foreground' : 'text-destructive'"
          >
            {{
              !draft.mac_compat
                ? t('fileServices.timeMachineNeedsMac')
                : timeMachineValid
                  ? t('fileServices.timeMachineDescription')
                  : t('fileServices.timeMachineTooSmall')
            }}
          </p>
        </SettingRow>
      </div>
    </template>

    <ComingLater
      v-else-if="isSoon(tab)"
      :icon="soon[tab]"
      :title="tabs.find((entry) => entry.id === tab)!.label"
      :description="t(`fileServices.soon.${tab}`)"
    />

    <PageFooter
      v-if="draft"
      :dirty="dirty"
      :saving="setServices.isPending.value"
      :valid="workgroupValid && timeMachineValid"
      :error="setServices.error.value && !advanced ? problemMessage(setServices.error.value) : null"
      @reset="reset()"
      @apply="apply()"
    />
    <SmbOptionsDialog
      v-if="draft"
      v-model:open="advanced"
      :services="draft"
      :saving="setServices.isPending.value"
      :error="setServices.error.value ? problemMessage(setServices.error.value) : null"
      @save="apply"
    />
  </div>
</template>

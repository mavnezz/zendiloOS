<script setup lang="ts">
import { ShieldAlert } from 'lucide-vue-next'
import { computed, reactive, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { CatalogAppDetail } from '@/shared/api/types'
import { problemMessage } from '@/shared/i18n'
import { Alert } from '@/shared/ui/alert'
import { Button } from '@/shared/ui/button'
import { ModalDialog } from '@/shared/ui/dialog'
import { Input } from '@/shared/ui/input'
import { Label } from '@/shared/ui/label'

import { useInstallActions, useShareNames } from '../api'
import { appText } from '../texts'

const props = defineProps<{ app: CatalogAppDetail }>()
const open = defineModel<boolean>('open', { default: false })
const emit = defineEmits<{ installed: [id: string] }>()

const { t, locale } = useI18n()
const actions = useInstallActions()
const { data: shareNames } = useShareNames()

const ports = reactive<Record<string, string>>({})
const shares = reactive<Record<string, string>>({})
const settings = reactive<Record<string, string>>({})
const confirmed = ref(false)
const busy = ref(false)
const failure = ref<string | null>(null)

const asked = computed(() => props.app.settings.filter((setting) => !setting.generate))
const generated = computed(() => props.app.settings.filter((setting) => setting.generate))
const permissions = computed(() => props.app.permissions)
const asksMore = computed(
  () =>
    permissions.value.devices.length > 0 ||
    permissions.value.capabilities.length > 0 ||
    permissions.value.host_network,
)
const ready = computed(
  () =>
    !busy.value &&
    (!asksMore.value || confirmed.value) &&
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
    }
    confirmed.value = false
    failure.value = null
  },
  { immediate: true },
)

async function install(): Promise<void> {
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
      confirm_permissions: confirmed.value,
    })
    open.value = false
    emit('installed', props.app.id)
  } catch (problem) {
    failure.value = problemMessage(problem)
  } finally {
    busy.value = false
  }
}

const field = 'h-9 rounded-md border bg-background px-3 text-sm shadow-xs'
</script>

<!-- What an app gets from the NAS, asked once before it is installed. -->
<template>
  <ModalDialog
    v-model:open="open"
    :title="t('appStore.installTitle', { name: appText(app, 'name', locale) })"
    :description="t('appStore.installHint')"
    :close-label="t('desktop.close')"
  >
    <form class="grid gap-4" @submit.prevent="install">
      <Alert v-if="!app.official" class="border-amber-500/60">
        <ShieldAlert class="text-amber-600" />
        {{ t('appStore.unofficialHint') }}
      </Alert>

      <div v-for="share in app.shares" :key="`share-${share.id}`" class="grid gap-2">
        <Label :for="`share-${share.id}`">
          {{ share.label }}
          <span class="font-normal text-muted-foreground">
            ({{ share.read_write ? t('appStore.readWrite') : t('appStore.readOnly') }})
          </span>
        </Label>
        <select :id="`share-${share.id}`" v-model="shares[share.id]" required :class="field">
          <option value="" disabled>{{ t('appStore.pickShare') }}</option>
          <option v-for="name in shareNames" :key="name" :value="name">{{ name }}</option>
        </select>
        <p v-if="share.description" class="text-xs text-muted-foreground">
          {{ share.description }}
        </p>
        <p v-if="shareNames && !shareNames.length" class="text-xs text-destructive">
          {{ t('appStore.noShares') }}
        </p>
      </div>

      <div v-for="port in app.ports" :key="`port-${port.id}`" class="grid gap-2">
        <Label :for="`port-${port.id}`">{{ t('appStore.portLabel', { label: port.label }) }}</Label>
        <Input
          :id="`port-${port.id}`"
          v-model="ports[port.id]"
          type="number"
          min="1"
          max="65535"
          required
          :disabled="port.fixed"
        />
        <p v-if="port.fixed" class="text-xs text-muted-foreground">{{ t('appStore.fixedHint') }}</p>
      </div>

      <div v-for="setting in asked" :key="`setting-${setting.id}`" class="grid gap-2">
        <label v-if="setting.kind === 'boolean'" class="flex items-center gap-2 text-sm">
          <input
            :id="`setting-${setting.id}`"
            type="checkbox"
            class="size-4 accent-primary"
            :checked="settings[setting.id] === 'true'"
            @change="
              settings[setting.id] = ($event.target as HTMLInputElement).checked ? 'true' : 'false'
            "
          />
          {{ setting.label }}
        </label>
        <template v-else>
          <Label :for="`setting-${setting.id}`">
            {{ setting.label }}
            <span v-if="!setting.required" class="font-normal text-muted-foreground">
              ({{ t('appStore.optional') }})
            </span>
          </Label>
          <Input
            :id="`setting-${setting.id}`"
            v-model="settings[setting.id]"
            :type="
              setting.kind === 'password'
                ? 'password'
                : setting.kind === 'number'
                  ? 'number'
                  : 'text'
            "
            :required="setting.required"
            autocomplete="off"
          />
        </template>
        <p v-if="setting.description" class="text-xs text-muted-foreground">
          {{ setting.description }}
        </p>
      </div>

      <p v-if="generated.length" class="text-xs text-muted-foreground">
        {{
          t(
            'appStore.generated',
            { labels: generated.map((setting) => setting.label).join(', ') },
            generated.length,
          )
        }}
      </p>

      <div v-if="asksMore" class="grid gap-2 rounded-xl border border-amber-500/60 p-3 text-sm">
        <p class="font-medium">{{ t('appStore.permissionsTitle') }}</p>
        <ul class="grid gap-1 text-muted-foreground">
          <li v-for="device in permissions.devices" :key="device">
            {{ t('appStore.deviceExplained', { device }) }}
          </li>
          <li v-for="capability in permissions.capabilities" :key="capability">
            {{ t('appStore.capabilityExplained', { capability }) }}
          </li>
          <li v-if="permissions.host_network">{{ t('appStore.hostNetworkExplained') }}</li>
        </ul>
        <label class="flex items-start gap-2">
          <input v-model="confirmed" type="checkbox" class="mt-0.5 size-4 accent-primary" />
          <span>{{ t('appStore.confirmPermissions') }}</span>
        </label>
      </div>

      <Alert v-if="failure" variant="destructive">{{ failure }}</Alert>
      <div class="flex justify-end gap-2">
        <Button type="button" variant="outline" @click="open = false">{{
          t('desktop.cancel')
        }}</Button>
        <Button type="submit" :disabled="!ready">{{ t('appStore.install') }}</Button>
      </div>
    </form>
  </ModalDialog>
</template>

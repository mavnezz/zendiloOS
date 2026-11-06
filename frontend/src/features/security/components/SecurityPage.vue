<script setup lang="ts">
import { BrickWall, FileBadge, ShieldBan, UserRoundSearch } from 'lucide-vue-next'
import { computed, ref, shallowRef, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import { problemMessage } from '@/shared/i18n'
import { Badge } from '@/shared/ui/badge'
import { Button } from '@/shared/ui/button'
import { Input } from '@/shared/ui/input'
import { NativeSelect } from '@/shared/ui/native-select'
import { ComingLater, PageFooter, PageHeader, PageTabs, SettingRow } from '@/shared/ui/page'
import { Switch } from '@/shared/ui/switch'

import { useBlocked, useSecuritySettings, useSetSecuritySettings, useSetTls, useTls } from '../api'
import { fromDraft, invalid, sameSettings, type SecurityDraft, toDraft } from '../lib/settings'
import AccountActivityDialog from './AccountActivityDialog.vue'
import BlockedAddressesDialog from './BlockedAddressesDialog.vue'

const { t } = useI18n()
const { data: settings, isPending } = useSecuritySettings()
const { data: tls } = useTls()
const { data: blocked } = useBlocked()
const setSettings = useSetSecuritySettings()
const setTls = useSetTls()

const tab = ref('general')
const blocking = ref(false)
const activity = ref(false)
const draft = ref<SecurityDraft>()
/** The settings the draft started from. */
const base = shallowRef<SecurityDraft>()

function reset(): void {
  if (!settings.value || !tls.value) return
  base.value = toDraft(settings.value, tls.value.minimum)
  draft.value = toDraft(settings.value, tls.value.minimum)
  setSettings.reset()
  setTls.reset()
}
const settingsDirty = computed(() =>
  Boolean(draft.value && base.value && !sameSettings(draft.value, base.value)),
)
const tlsDirty = computed(() =>
  Boolean(draft.value && base.value && draft.value.tls !== base.value.tls),
)
const dirty = computed(() => settingsDirty.value || tlsDirty.value)
watch(
  [settings, tls],
  () => {
    if (!base.value || !dirty.value) reset()
  },
  { immediate: true },
)

const wrong = computed(() => (draft.value ? invalid(draft.value) : new Set()))
const saving = computed(() => setSettings.isPending.value || setTls.isPending.value)
const error = computed(() => setSettings.error.value ?? setTls.error.value)

async function apply(): Promise<void> {
  const value = draft.value
  if (!value || !settings.value || wrong.value.size) return
  try {
    if (settingsDirty.value) {
      await setSettings.mutateAsync(fromDraft(value, settings.value.allowed ?? []))
    }
    if (tlsDirty.value) await setTls.mutateAsync({ minimum: value.tls })
    reset()
  } catch {
    // Shown in the footer.
  }
}

const tabs = computed(() => [
  { id: 'general', label: t('security.tabs.general') },
  { id: 'accounts', label: t('security.tabs.accounts') },
  { id: 'firewall', label: t('security.tabs.firewall') },
  { id: 'certificate', label: t('security.tabs.certificate') },
])

const number = 'w-20 font-mono text-[13px]'
const line = 'flex flex-wrap items-center gap-2 text-sm'
</script>

<!--
  How the web interface defends itself, as on the appliances people know:
  idle sessions, blocked addresses and TLS on the first tab, sessions and
  locked accounts on the second. Firewall and certificate come later.
-->
<template>
  <div class="flex min-h-full flex-col">
    <PageHeader :title="t('security.title')" :description="t('security.description')" />
    <PageTabs v-model="tab" :label="t('security.title')" :tabs="tabs" />

    <div
      v-if="(tab === 'general' || tab === 'accounts') && (isPending || !draft)"
      class="h-64 max-w-2xl animate-pulse rounded-lg bg-muted"
    />

    <div v-else-if="tab === 'general' && draft" class="grid max-w-3xl gap-6">
      <SettingRow :label="t('security.idle')" for="security-idle">
        <div :class="line">
          {{ t('security.idleBefore') }}
          <Input
            id="security-idle"
            v-model="draft.idle_minutes"
            inputmode="numeric"
            :class="number"
            :aria-invalid="wrong.has('idle_minutes')"
          />
          {{ t('security.idleAfter') }}
        </div>
        <p
          class="text-xs"
          :class="wrong.has('idle_minutes') ? 'text-destructive' : 'text-muted-foreground'"
        >
          {{ t('security.idleHint') }}
        </p>
      </SettingRow>

      <SettingRow :label="t('security.addressBlock')" for="security-block">
        <label class="flex items-center gap-3 pt-1 text-sm">
          <Switch id="security-block" v-model="draft.block" />
          {{ t('security.blockOn') }}
        </label>
        <div :class="line" class="mt-1">
          <Input
            v-model="draft.block_failures"
            inputmode="numeric"
            :class="number"
            :disabled="!draft.block"
            :aria-label="t('security.failures')"
            :aria-invalid="wrong.has('block_failures')"
          />
          {{ t('security.failuresWithin') }}
          <Input
            v-model="draft.block_window"
            inputmode="numeric"
            :class="number"
            :disabled="!draft.block"
            :aria-label="t('security.window')"
            :aria-invalid="wrong.has('block_window')"
          />
          {{ t('security.minutes') }}
        </div>
        <div :class="line">
          <Switch id="security-release" v-model="draft.release" :disabled="!draft.block" />
          <label for="security-release">{{ t('security.releaseAfter') }}</label>
          <Input
            v-model="draft.release_days"
            inputmode="numeric"
            :class="number"
            :disabled="!draft.block || !draft.release"
            :aria-label="t('security.releaseDays')"
            :aria-invalid="wrong.has('release_days')"
          />
          {{ t('security.days') }}
        </div>
        <p class="text-xs text-muted-foreground">{{ t('security.blockHint') }}</p>
        <Button variant="secondary" class="mt-1 w-fit" @click="blocking = true">
          <ShieldBan />
          {{ t('security.manageBlocks') }}
          <Badge v-if="blocked?.length" variant="destructive">{{ blocked.length }}</Badge>
        </Button>
      </SettingRow>

      <SettingRow :label="t('security.tls')" for="security-tls">
        <NativeSelect id="security-tls" v-model="draft.tls" class="w-72">
          <option value="tls1_2">{{ t('security.tls12') }}</option>
          <option value="tls1_3">{{ t('security.tls13') }}</option>
        </NativeSelect>
        <p class="text-xs text-muted-foreground">{{ t('security.tlsHint') }}</p>
      </SettingRow>
    </div>

    <div v-else-if="tab === 'accounts' && draft" class="grid max-w-3xl gap-6">
      <SettingRow :label="t('security.activity.title')">
        <p class="pt-1.5 text-sm text-muted-foreground">{{ t('security.activityHint') }}</p>
        <Button variant="secondary" class="w-fit" @click="activity = true">
          <UserRoundSearch />
          {{ t('security.manage') }}
        </Button>
      </SettingRow>

      <SettingRow :label="t('security.accountLock')" for="security-lock">
        <label class="flex items-center gap-3 pt-1 text-sm">
          <Switch id="security-lock" v-model="draft.lock" />
          {{ t('security.lockOn') }}
        </label>
        <div :class="line" class="mt-1">
          <Input
            v-model="draft.lock_failures"
            inputmode="numeric"
            :class="number"
            :disabled="!draft.lock"
            :aria-label="t('security.failures')"
            :aria-invalid="wrong.has('lock_failures')"
          />
          {{ t('security.failuresWithin') }}
          <Input
            v-model="draft.lock_window"
            inputmode="numeric"
            :class="number"
            :disabled="!draft.lock"
            :aria-label="t('security.window')"
            :aria-invalid="wrong.has('lock_window')"
          />
          {{ t('security.minutes') }}
        </div>
        <p class="text-xs text-muted-foreground">{{ t('security.lockHint') }}</p>
      </SettingRow>
    </div>

    <ComingLater
      v-else-if="tab === 'firewall'"
      :icon="BrickWall"
      :title="t('security.tabs.firewall')"
      :description="t('security.soon.firewall')"
    />
    <ComingLater
      v-else-if="tab === 'certificate'"
      :icon="FileBadge"
      :title="t('security.tabs.certificate')"
      :description="t('security.soon.certificate')"
    />

    <div class="grow" aria-hidden="true" />
    <PageFooter
      v-if="draft && (tab === 'general' || tab === 'accounts')"
      :dirty="dirty"
      :saving="saving"
      :valid="wrong.size === 0"
      :error="error ? problemMessage(error) : null"
      @reset="reset()"
      @apply="apply()"
    />
    <BlockedAddressesDialog v-if="settings" v-model:open="blocking" :settings="settings" />
    <AccountActivityDialog v-model:open="activity" />
  </div>
</template>

<script setup lang="ts">
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { useSessionStore } from '@/features/auth/store'
import ClockSettings from '@/features/system/components/ClockSettings.vue'
import { LANGUAGES, type Locale } from '@/shared/i18n'
import { Label } from '@/shared/ui/label'
import { NativeSelect } from '@/shared/ui/native-select'
import { PageDescription, PageHeader, PageTabs } from '@/shared/ui/page'
import { usePreferences } from '@/shared/preferences/store'

const { t, locale } = useI18n()
const session = useSessionStore()
const preferences = usePreferences()

/** The clock belongs to the machine, the language to the account. */
const tabs = computed(() => [
  ...(session.can('system.view') ? [{ id: 'time', label: t('locale.time') }] : []),
  { id: 'language', label: t('locale.language') },
])
const tab = ref(tabs.value[0]!.id)

const language = computed({
  get: () => locale.value,
  set: (value: string) => preferences.chooseLanguage(value as Locale),
})
</script>

<!-- Time and language: two different things that share a page, as on the appliances people know. -->
<template>
  <div class="flex min-h-full flex-col">
    <PageHeader :title="t('controlPanel.locale')" />
    <PageTabs v-model="tab" :label="t('controlPanel.locale')" :tabs="tabs" />
    <ClockSettings v-if="tab === 'time'" :can-manage="session.can('system.manage')" />
    <template v-else>
      <div class="grid max-w-xl gap-2">
        <Label for="locale-language">{{ t('locale.language') }}</Label>
        <NativeSelect id="locale-language" v-model="language" class="max-w-xs">
          <option v-for="entry in LANGUAGES" :key="entry.locale" :value="entry.locale">
            {{ entry.name }}
          </option>
        </NativeSelect>
      </div>
      <PageDescription>
        <p>{{ t('locale.languageDescription') }}</p>
      </PageDescription>
    </template>
  </div>
</template>

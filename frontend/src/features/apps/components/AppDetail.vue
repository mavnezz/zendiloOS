<script setup lang="ts">
import { ArrowLeft, ExternalLink, ShieldAlert } from 'lucide-vue-next'
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { problemMessage } from '@/shared/i18n'
import { Alert } from '@/shared/ui/alert'
import { Badge } from '@/shared/ui/badge'
import { Button } from '@/shared/ui/button'

import { iconUrl, useCatalogApp, useDocker, useInstalled } from '../api'
import { appText } from '../texts'
import InstallDialog from './InstallDialog.vue'

const props = defineProps<{ source: number; id: string }>()
const emit = defineEmits<{ back: []; installed: [] }>()

const { t, locale } = useI18n()
const which = computed(() => ({ source: props.source, id: props.id }))
const { data: app, error } = useCatalogApp(which)

const { data: installed } = useInstalled()
const { data: docker } = useDocker()
const installing = ref(false)

/** The app installed from any catalog: one app per id on the NAS. */
const present = computed(() => installed.value?.find((other) => other.id === props.id))
const dockerReady = computed(() => Boolean(docker.value?.volume && docker.value.running))

const asked = computed(() => (app.value?.settings ?? []).filter((setting) => !setting.generate))
const generated = computed(() => (app.value?.settings ?? []).filter((setting) => setting.generate))
</script>

<!-- One app: what it is, and what it needs from the NAS. -->
<template>
  <div class="grid gap-5">
    <Button variant="ghost" size="sm" class="w-fit" @click="emit('back')">
      <ArrowLeft />
      {{ t('appStore.back') }}
    </Button>
    <Alert v-if="error" variant="destructive">{{ problemMessage(error) }}</Alert>
    <template v-if="app">
      <header class="flex items-start gap-4">
        <img :src="iconUrl(app)" alt="" class="size-16 shrink-0 rounded-xl" />
        <div class="grid min-w-0 flex-1 gap-1">
          <h3 class="text-lg font-semibold">{{ appText(app, 'name', locale) }}</h3>
          <p class="text-sm text-muted-foreground">{{ appText(app, 'tagline', locale) }}</p>
          <p class="text-xs text-muted-foreground">
            {{ t('appStore.by', { developer: app.developer, version: app.version }) }}
          </p>
        </div>
        <Button v-if="present" variant="outline" @click="emit('installed')">
          {{ t('appStore.isInstalled') }}
        </Button>
        <Button
          v-else
          :disabled="!dockerReady"
          :title="dockerReady ? undefined : t('appStore.dockerFirst')"
          @click="installing = true"
        >
          {{ t('appStore.install') }}
        </Button>
      </header>

      <Alert v-if="!app.official" class="border-amber-500/60">
        <ShieldAlert class="text-amber-600" />
        {{ t('appStore.unofficialHint') }}
      </Alert>

      <p class="text-sm whitespace-pre-line">{{ appText(app, 'description', locale) }}</p>
      <Alert v-if="appText(app, 'notes', locale)">
        <span class="whitespace-pre-line">{{ appText(app, 'notes', locale) }}</span>
      </Alert>

      <section class="grid gap-2 text-sm">
        <h4 class="font-medium">{{ t('appStore.needs') }}</h4>
        <ul class="grid gap-1.5">
          <li v-for="port in app.ports" :key="`port-${port.id}`">
            {{ t('appStore.port', { label: port.label, port: port.default }) }}
            <span v-if="port.fixed" class="text-muted-foreground">({{ t('appStore.fixed') }})</span>
          </li>
          <li v-for="share in app.shares" :key="`share-${share.id}`">
            {{ t('appStore.share', { label: share.label }) }}
            <Badge variant="secondary">
              {{ share.read_write ? t('appStore.readWrite') : t('appStore.readOnly') }}
            </Badge>
            <span v-if="share.description" class="block text-xs text-muted-foreground">
              {{ share.description }}
            </span>
          </li>
          <li v-for="setting in asked" :key="`setting-${setting.id}`">
            {{ t('appStore.setting', { label: setting.label }) }}
          </li>
          <li v-if="generated.length" class="text-muted-foreground">
            {{
              t(
                'appStore.generated',
                { labels: generated.map((s) => s.label).join(', ') },
                generated.length,
              )
            }}
          </li>
        </ul>
      </section>

      <section
        v-if="
          app.permissions.devices.length ||
          app.permissions.capabilities.length ||
          app.permissions.host_network
        "
        class="grid gap-2 text-sm"
      >
        <h4 class="font-medium">{{ t('appStore.permissionsTitle') }}</h4>
        <ul class="grid gap-1.5">
          <li v-for="device in app.permissions.devices" :key="device">
            {{ t('appStore.deviceExplained', { device }) }}
          </li>
          <li v-for="capability in app.permissions.capabilities" :key="capability">
            {{ t('appStore.capabilityExplained', { capability }) }}
          </li>
          <li v-if="app.permissions.host_network">{{ t('appStore.hostNetworkExplained') }}</li>
        </ul>
      </section>

      <dl class="grid grid-cols-[auto_1fr] gap-x-6 gap-y-1.5 text-sm">
        <dt class="text-muted-foreground">{{ t('appStore.license') }}</dt>
        <dd>{{ app.license }}</dd>
        <dt class="text-muted-foreground">{{ t('appStore.architectures') }}</dt>
        <dd>{{ app.architectures.join(', ') }}</dd>
        <dt class="text-muted-foreground">{{ t('appStore.links') }}</dt>
        <dd class="flex flex-wrap gap-3">
          <a
            :href="app.website"
            target="_blank"
            rel="noopener noreferrer"
            class="inline-flex items-center gap-1 underline"
          >
            {{ t('appStore.website') }} <ExternalLink class="size-3" />
          </a>
          <a
            :href="app.source_code"
            target="_blank"
            rel="noopener noreferrer"
            class="inline-flex items-center gap-1 underline"
          >
            {{ t('appStore.sourceCode') }} <ExternalLink class="size-3" />
          </a>
        </dd>
      </dl>

      <InstallDialog v-model:open="installing" :app="app" @installed="emit('installed')" />

      <details class="text-sm">
        <summary class="cursor-pointer text-muted-foreground">{{ t('appStore.compose') }}</summary>
        <pre
          class="mt-2 max-h-80 overflow-auto rounded-md border bg-muted/40 p-3 font-mono text-xs"
          >{{ app.compose }}</pre>
      </details>
    </template>
  </div>
</template>

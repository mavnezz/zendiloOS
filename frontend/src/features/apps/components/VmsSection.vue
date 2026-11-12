<script setup lang="ts">
import { useQueryClient } from '@tanstack/vue-query'
import { ChevronDown, Cpu, KeyRound, LoaderCircle, MonitorCog, ShieldCheck } from 'lucide-vue-next'
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import { problemMessage } from '@/shared/i18n'
import { useJobsStore } from '@/shared/jobs/store'
import { formatBytes } from '@/shared/lib/format'
import { useOpenApp } from '@/shared/lib/window-section'
import { Alert } from '@/shared/ui/alert'
import { Badge } from '@/shared/ui/badge'
import { Button } from '@/shared/ui/button'
import { ModalDialog } from '@/shared/ui/dialog'
import { Label } from '@/shared/ui/label'

import { installVms, setUpVmStorage, useVmPlugin, vmsReady } from '../vms'

const { t } = useI18n()
const client = useQueryClient()
const jobs = useJobsStore()
const openApp = useOpenApp()
const { data: plugin, error } = useVmPlugin()

const choosing = ref(false)
const pool = ref('')
const busy = ref(false)
const failure = ref<string | null>(null)

/** The latest installation of libvirt and QEMU, running or done. */
const job = computed(() => jobs.list.find((job) => job.kind === 'vms.install'))
const installing = computed(() => job.value?.state === 'running')
const failed = computed(() => (job.value?.state === 'failed' ? job.value : null))
const kvm = computed(() => plugin.value?.hardware.kvm === true)
const ready = computed(() => vmsReady(plugin.value))
/** Installed before VMs had a TPM: installing again brings swtpm. */
const lacksTpm = computed(() => plugin.value?.installed === true && !plugin.value.hardware.swtpm)

// Once a run ends, what the plugin is now.
watch(
  () => job.value?.state,
  (state, before) => {
    if (before === 'running' && state !== 'running') {
      void client.invalidateQueries({ queryKey: ['vms'] })
    }
  },
)

const flag = computed(() => {
  const name = plugin.value?.hardware.cpu_virtualization
  return name === 'vmx' || name === 'svm'
    ? t(`appStore.vms.flags.${name}`)
    : t('appStore.vms.flags.none')
})
const place = computed(() =>
  plugin.value?.storage
    ? t('appStore.placeVolume', {
        number: plugin.value.storage.volume,
        pool: plugin.value.storage.pool,
      })
    : t('appStore.vms.notSetUp'),
)

async function install(): Promise<void> {
  failure.value = null
  try {
    jobs.update(await installVms())
  } catch (problem) {
    failure.value = problemMessage(problem)
  }
}

function choose(): void {
  failure.value = null
  const pools = [...(plugin.value?.pools ?? [])].sort((a, b) => a.volume - b.volume)
  pool.value = pools[0]?.pool ?? ''
  choosing.value = true
}

async function putThere(): Promise<void> {
  if (!pool.value) return
  busy.value = true
  failure.value = null
  try {
    client.setQueryData(['vms', 'plugin'], await setUpVmStorage(pool.value))
    choosing.value = false
  } catch (problem) {
    failure.value = problemMessage(problem)
  } finally {
    busy.value = false
  }
}

const fact = 'grid gap-1 px-3 text-center not-first:border-l'
const heading = 'text-base font-semibold'
const select =
  'h-10 w-full appearance-none rounded-lg border-transparent bg-foreground/[0.05] px-3 pr-9 text-sm outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50'
</script>

<!-- Virtual machines: the hardware checked, libvirt and QEMU installed, the storage on a pool. -->
<template>
  <div class="grid gap-6">
    <Alert v-if="error" variant="destructive">{{ problemMessage(error) }}</Alert>
    <header class="flex flex-wrap items-center gap-5 px-2">
      <span
        class="grid size-20 shrink-0 place-items-center rounded-2xl bg-linear-to-br from-violet-400 to-indigo-600 text-white shadow-sm"
      >
        <MonitorCog class="size-10" />
      </span>
      <div class="grid min-w-0 flex-1 gap-1">
        <h2 class="text-xl font-semibold">{{ t('appStore.vms.name') }}</h2>
        <p class="text-sm text-muted-foreground">{{ t('appStore.vms.subtitle') }}</p>
      </div>
      <span v-if="installing" class="flex items-center gap-2 text-sm text-muted-foreground">
        <LoaderCircle class="size-4 animate-spin" />
        {{ t('appStore.vms.installing') }}
      </span>
      <Button
        v-else-if="plugin && !plugin.installed"
        size="lg"
        class="rounded-full px-6"
        :disabled="!kvm"
        @click="install"
        >{{ t('appStore.install') }}</Button
      >
      <Button
        v-else-if="plugin && !plugin.storage"
        size="lg"
        class="rounded-full px-6"
        :disabled="!plugin.pools.length"
        @click="choose"
        >{{ t('appStore.vms.choosePlace') }}</Button
      >
      <Button
        v-else-if="ready && openApp"
        size="lg"
        variant="outline"
        class="rounded-full px-6"
        @click="openApp('vms')"
        >{{ t('appStore.vms.toApp') }}</Button
      >
    </header>

    <section class="grid gap-3" aria-labelledby="vms-hardware">
      <h3 id="vms-hardware" :class="heading" class="px-2">{{ t('appStore.vms.hardware') }}</h3>
      <ul class="grid gap-2 text-sm">
        <li class="flex items-start gap-3 rounded-lg border p-3" data-testid="vms-kvm">
          <Cpu class="mt-0.5 size-4 shrink-0 text-muted-foreground" />
          <span class="grid min-w-0 flex-1 gap-0.5">
            <span class="flex flex-wrap items-center gap-2 font-medium">
              {{ t('appStore.vms.kvm') }}
              <Badge v-if="plugin" :variant="kvm ? 'success' : 'destructive'">{{ flag }}</Badge>
            </span>
            <span class="text-muted-foreground">{{
              kvm ? t('appStore.vms.kvmOn', { flag }) : t('appStore.vms.kvmOff')
            }}</span>
          </span>
        </li>
        <li class="flex items-start gap-3 rounded-lg border p-3" data-testid="vms-iommu">
          <Cpu class="mt-0.5 size-4 shrink-0 text-muted-foreground" />
          <span class="grid min-w-0 flex-1 gap-0.5">
            <span class="flex flex-wrap items-center gap-2 font-medium">
              {{ t('appStore.vms.iommu') }}
              <Badge v-if="plugin" :variant="plugin.hardware.iommu ? 'success' : 'secondary'">{{
                plugin.hardware.iommu ? t('appStore.vms.ready') : '–'
              }}</Badge>
            </span>
            <span class="text-muted-foreground">{{
              plugin?.hardware.iommu ? t('appStore.vms.iommuOn') : t('appStore.vms.iommuOff')
            }}</span>
            <span
              v-if="plugin && !plugin.hardware.iommu"
              class="text-xs text-muted-foreground"
              data-testid="vms-iommu-hint"
              >{{ t('appStore.vms.iommuHint') }}</span
            >
          </span>
        </li>
        <li
          v-if="plugin?.installed"
          class="flex items-start gap-3 rounded-lg border p-3"
          data-testid="vms-tpm"
        >
          <KeyRound class="mt-0.5 size-4 shrink-0 text-muted-foreground" />
          <span class="grid min-w-0 flex-1 gap-0.5">
            <span class="flex flex-wrap items-center gap-2 font-medium">
              {{ t('appStore.vms.tpm') }}
              <Badge :variant="plugin.hardware.swtpm ? 'success' : 'warning'">{{
                plugin.hardware.swtpm ? t('appStore.vms.ready') : '–'
              }}</Badge>
            </span>
            <span class="text-muted-foreground">{{
              plugin.hardware.swtpm ? t('appStore.vms.tpmOn') : t('appStore.vms.tpmOff')
            }}</span>
          </span>
          <Button
            v-if="lacksTpm && !installing"
            size="sm"
            variant="outline"
            data-testid="vms-reinstall"
            @click="install"
            >{{ t('appStore.vms.reinstall') }}</Button
          >
        </li>
        <li
          v-if="plugin?.installed"
          class="flex items-start gap-3 rounded-lg border p-3"
          data-testid="vms-secure-boot"
        >
          <ShieldCheck class="mt-0.5 size-4 shrink-0 text-muted-foreground" />
          <span class="grid min-w-0 flex-1 gap-0.5">
            <span class="flex flex-wrap items-center gap-2 font-medium">
              {{ t('appStore.vms.secureBoot') }}
              <Badge :variant="plugin.hardware.secure_boot ? 'success' : 'warning'">{{
                plugin.hardware.secure_boot ? t('appStore.vms.ready') : '–'
              }}</Badge>
            </span>
            <span class="text-muted-foreground">{{
              plugin.hardware.secure_boot
                ? t('appStore.vms.secureBootOn')
                : t('appStore.vms.secureBootOff')
            }}</span>
          </span>
        </li>
      </ul>
      <p v-if="lacksTpm" class="px-2 text-xs text-muted-foreground">
        {{ t('appStore.vms.reinstallHint') }}
      </p>
    </section>

    <dl class="grid grid-cols-2 gap-y-4 border-y py-4 text-sm @2xl:grid-cols-4">
      <div :class="fact">
        <dt class="text-xs text-muted-foreground">{{ t('appStore.docker.status') }}</dt>
        <dd class="font-medium">
          {{
            !plugin
              ? '–'
              : ready
                ? t('appStore.vms.ready')
                : plugin.installed
                  ? t('appStore.plugins.needsSetUp')
                  : t('appStore.vms.notInstalled')
          }}
        </dd>
      </div>
      <div :class="fact">
        <dt class="text-xs text-muted-foreground">{{ t('appStore.vms.host') }}</dt>
        <dd>
          {{
            plugin
              ? t('appStore.vms.hostValue', {
                  cores: plugin.cores,
                  memory: formatBytes(plugin.memory_bytes),
                })
              : '–'
          }}
        </dd>
      </div>
      <div :class="fact">
        <dt class="text-xs text-muted-foreground">{{ t('appStore.vms.storage') }}</dt>
        <dd class="truncate">{{ place }}</dd>
      </div>
      <div :class="fact">
        <dt class="text-xs text-muted-foreground">{{ t('appStore.docker.source') }}</dt>
        <dd>Debian</dd>
      </div>
    </dl>

    <Alert v-if="plugin && !kvm && !plugin.installed" variant="destructive">{{
      t('appStore.vms.noKvm')
    }}</Alert>
    <Alert v-if="plugin?.installed && !plugin.storage && !plugin.pools.length">
      <MonitorCog />
      {{ t('appStore.vms.noPool') }}
    </Alert>
    <Alert v-if="failure" variant="destructive">{{ failure }}</Alert>
    <div v-if="failed" class="grid gap-2">
      <Alert variant="destructive">{{ t('appStore.vms.failed') }}</Alert>
      <pre
        v-if="failed.error_detail"
        class="max-h-60 overflow-auto rounded-md bg-zinc-900 p-3 font-mono text-xs whitespace-pre-wrap text-zinc-100"
        >{{ failed.error_detail }}</pre>
    </div>

    <section class="grid gap-2 border-b pb-6">
      <h3 :class="heading">{{ t('appStore.description') }}</h3>
      <p class="text-sm leading-relaxed text-muted-foreground">{{ t('appStore.vms.about') }}</p>
      <p class="text-sm leading-relaxed text-muted-foreground">
        {{ t('appStore.vms.aboutNetwork') }}
      </p>
      <p class="text-sm leading-relaxed text-muted-foreground">
        {{ t('appStore.vms.proofOfConcept') }}
      </p>
    </section>
    <section class="grid gap-2 text-sm">
      <h3 :class="heading">{{ t('appStore.needs') }}</h3>
      <ul class="grid list-disc gap-1.5 pl-5 text-muted-foreground">
        <li>{{ t('appStore.vms.needsKvm') }}</li>
        <li>{{ t('appStore.vms.needsPackages') }}</li>
        <li>{{ t('appStore.vms.needsPool') }}</li>
      </ul>
    </section>

    <ModalDialog
      v-model:open="choosing"
      :title="t('appStore.placeTitle')"
      :close-label="t('desktop.close')"
    >
      <form id="vms-place" class="grid gap-3" @submit.prevent="putThere">
        <Label for="vms-pool" class="leading-normal font-normal">{{
          t('appStore.vms.placeQuestion')
        }}</Label>
        <div class="relative">
          <select id="vms-pool" v-model="pool" :class="select">
            <option v-for="choice in plugin?.pools ?? []" :key="choice.pool" :value="choice.pool">
              {{ t('appStore.placeVolume', { number: choice.volume, pool: choice.pool }) }}
            </option>
          </select>
          <ChevronDown
            class="pointer-events-none absolute top-1/2 right-3 size-4 -translate-y-1/2 text-muted-foreground"
          />
        </div>
        <p class="pb-4 text-xs leading-relaxed text-muted-foreground">
          {{ t('appStore.vms.placeHint') }}
        </p>
        <Alert v-if="failure" variant="destructive">{{ failure }}</Alert>
      </form>
      <template #footer>
        <Button type="button" variant="secondary" @click="choosing = false">{{
          t('desktop.cancel')
        }}</Button>
        <Button type="submit" form="vms-place" :disabled="!pool || busy">{{
          t('appStore.vms.putThere')
        }}</Button>
      </template>
    </ModalDialog>
  </div>
</template>

<script setup lang="ts">
import { ShieldAlert } from 'lucide-vue-next'
import { useI18n } from 'vue-i18n'

import type { CatalogApp } from '@/shared/api/types'

import { capabilityKind, deviceKind } from '../texts'

defineProps<{ permissions: CatalogApp['permissions'] }>()

const { t } = useI18n()
</script>

<!-- What an app asks for beyond the rules, each in plain words; the Docker socket loudest. -->
<template>
  <ul class="grid list-disc gap-1 pl-4">
    <li v-for="device in permissions.devices" :key="device">
      {{ t(`appStore.permissionText.${deviceKind(device)}`, { device }) }}
    </li>
    <li v-for="capability in permissions.capabilities" :key="capability">
      {{ t(`appStore.permissionText.${capabilityKind(capability)}`, { capability }) }}
    </li>
    <li v-if="permissions.host_network">{{ t('appStore.permissionText.hostNetwork') }}</li>
  </ul>
  <p
    v-if="permissions.docker_socket"
    class="mt-1 flex items-start gap-2 rounded-lg bg-destructive/10 p-2.5 text-destructive"
  >
    <ShieldAlert class="mt-0.5 size-4 shrink-0" />
    <span>{{ t('appStore.permissionText.dockerSocket') }}</span>
  </p>
</template>

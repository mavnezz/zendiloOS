<script setup lang="ts">
import { useSessionStore } from '@/features/auth/store'
import StorageApp from '@/features/storage/components/StorageApp.vue'

const session = useSessionStore()
</script>

<template>
  <StorageApp :can-manage="session.can('storage.manage')" />
</template>

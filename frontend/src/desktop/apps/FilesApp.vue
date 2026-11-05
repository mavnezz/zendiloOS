<script setup lang="ts">
import { useSessionStore } from '@/features/auth/store'
import FileManager from '@/features/files/components/FileManager.vue'

const session = useSessionStore()
</script>

<template>
  <FileManager :user-name="session.info?.account.name" />
</template>

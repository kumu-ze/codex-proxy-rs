import type { PluginStatus } from '@/api/modules/plugins'
import { defineStore } from 'pinia'
import { ref } from 'vue'
import { getPlugins } from '@/api/modules/plugins'

export const usePluginsStore = defineStore('plugins', () => {
  const plugins = ref<PluginStatus[]>([])
  async function refresh() {
    plugins.value = await getPlugins()
  }
  return { plugins, refresh }
})

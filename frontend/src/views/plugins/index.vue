<script setup lang="ts">
import type { PluginStatus } from '@/api/modules/plugins'
import { useEventListener } from '@vueuse/core'
import { onMounted, ref } from 'vue'
import { getPlugins, invokePlugin } from '@/api/modules/plugins'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseCard from '@/components/base/BaseCard.vue'
import BasePageHeader from '@/components/base/BasePageHeader.vue'
import BaseTag from '@/components/base/BaseTag.vue'

const plugins = ref<PluginStatus[]>([])
const loading = ref(false)
const failure = ref('')
const selected = ref('')
const document = ref('')
const frame = ref<HTMLIFrameElement>()
let generation = 0
let pending = false

async function load() {
  loading.value = true
  failure.value = ''
  try {
    plugins.value = await getPlugins()
  }
  catch {
    failure.value = '无法读取插件列表，请稍后重试。'
  }
  finally {
    loading.value = false
  }
}

async function open(plugin: PluginStatus) {
  const current = ++generation
  selected.value = plugin.id
  document.value = ''
  failure.value = ''
  try {
    const data = await invokePlugin(plugin.id, 'admin.ui', {})
    if (current !== generation)
      return
    if (!data || typeof data !== 'object' || !('html' in data) || typeof data.html !== 'string' || data.html.length > 262144)
      throw new Error('Invalid plugin page')
    // iframe 使用不透明来源；网络与宿主身份只能通过受限桥接取得。
    document.value = `<meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data:; connect-src 'none'; form-action 'none'; base-uri 'none';">${data.html}`
  }
  catch {
    if (current === generation)
      failure.value = '插件页面不可用。请检查插件是否提供管理页面。'
  }
}

useEventListener(window, 'message', async (event: MessageEvent) => {
  const target = frame.value?.contentWindow
  if (!target || event.source !== target || !selected.value)
    return
  const message = event.data
  if (!message || message.type !== 'rs-plugin-call' || typeof message.id !== 'string' || message.id.length > 64 || typeof message.method !== 'string' || !/^admin\.[\w.-]{1,120}$/.test(message.method))
    return
  const current = generation
  if (pending) {
    target.postMessage({ type: 'rs-plugin-result', id: message.id, error: 'busy' }, '*')
    return
  }
  pending = true
  try {
    const data = await invokePlugin(selected.value, message.method, message.input ?? {})
    if (current === generation && target === frame.value?.contentWindow)
      target.postMessage({ type: 'rs-plugin-result', id: message.id, data }, '*')
  }
  catch {
    if (current === generation && target === frame.value?.contentWindow)
      target.postMessage({ type: 'rs-plugin-result', id: message.id, error: 'unavailable' }, '*')
  }
  finally {
    pending = false
  }
})

onMounted(load)
</script>

<template>
  <div class="flex flex-col gap-6">
    <BasePageHeader title="插件" description="管理已配置的扩展">
      <template #actions>
        <BaseButton :disabled="loading" @click="load">
          刷新
        </BaseButton>
      </template>
    </BasePageHeader>
    <p v-if="failure" role="alert" class="text-cp-error-text">
      {{ failure }}
    </p>
    <BaseCard class="p-5">
      <p v-if="!plugins.length" class="text-cp-text-secondary">
        {{ loading ? '正在读取插件…' : '尚未配置插件。安装后在服务配置中启用并重启。' }}
      </p>
      <div v-for="plugin in plugins" :key="plugin.id" class="flex flex-wrap items-center justify-between gap-4 py-3">
        <div>
          <h2 class="font-semibold">
            {{ plugin.id }}
          </h2>
          <span class="text-sm text-cp-text-secondary">{{ plugin.version }}</span>
        </div>
        <BaseTag :type="plugin.available ? 'success' : 'warning'">
          {{ plugin.available ? '可用' : '不可用' }}
        </BaseTag>
        <BaseButton :disabled="!plugin.available" @click="open(plugin)">
          打开
        </BaseButton>
      </div>
    </BaseCard>
    <iframe v-if="document" :key="generation" ref="frame" :title="`${selected} 管理页面`" :srcdoc="document" sandbox="allow-scripts" class="min-h-[36rem] w-full rounded-xl border-0 bg-cp-bg-container" />
  </div>
</template>

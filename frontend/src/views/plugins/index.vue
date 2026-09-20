<script setup lang="ts">
import type { PluginManagement, PluginStatus } from '@/api/modules/plugins'
import { useEventListener } from '@vueuse/core'
import { storeToRefs } from 'pinia'
import { computed, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { invokePlugin, managePlugin } from '@/api/modules/plugins'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseCard from '@/components/base/BaseCard.vue'
import BaseConfirmModal from '@/components/base/BaseConfirmModal.vue'
import FormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import BaseModal from '@/components/base/BaseModal/index.vue'
import BasePageHeader from '@/components/base/BasePageHeader.vue'
import BaseTag from '@/components/base/BaseTag.vue'
import { usePluginsStore } from '@/stores/modules/plugins'

const store = usePluginsStore()
const { plugins } = storeToRefs(store)
const route = useRoute()
const router = useRouter()
const pageId = computed(() => typeof route.params.id === 'string' ? route.params.id : '')
const busy = ref(false)
const failure = ref('')
const installOpen = ref(false)
const downloadUrl = ref('')
const checksum = ref('')
const confirmOpen = ref(false)
const target = ref<PluginStatus>()
const action = ref<'enable' | 'disable' | 'uninstall'>('disable')
const notice = ref('')
const frameHeight = ref(720)
const confirmation = computed(() => action.value === 'enable' ? '启用插件后，其程序将运行并参与请求处理。请只启用可信来源的插件。' : action.value === 'disable' ? '停用后插件进程和自动任务停止，新请求不再使用该插件提供的处理与保护。' : '移除插件及菜单入口，保留业务数据。重新安装后可以恢复使用。')
const actionLabel = computed(() => ({ enable: '启用', disable: '停用', uninstall: '卸载' })[action.value])
function ask(plugin: PluginStatus, next: 'enable' | 'disable' | 'uninstall') {
  target.value = plugin
  action.value = next
  confirmOpen.value = true
}
async function manage(operation: PluginManagement) {
  busy.value = true
  failure.value = ''
  notice.value = ''
  try {
    plugins.value = await managePlugin(operation)
    installOpen.value = false
    confirmOpen.value = false
    notice.value = operation.action === 'install' ? '安装完成。确认来源可信后，点击“启用”。' : '操作完成，状态已保存。'
    if (operation.action === 'install') {
      downloadUrl.value = ''
      checksum.value = ''
    }
  }
  catch (error) {
    failure.value = error instanceof Error ? error.message : '操作未完成，请刷新列表确认状态后重试。'
    await store.refresh().catch(() => {})
  }
  finally { busy.value = false }
}
function install() {
  void manage({ action: 'install', url: downloadUrl.value.trim(), sha256: checksum.value.trim() || undefined })
}
function confirm() {
  if (target.value)
    void manage({ action: action.value, id: target.value.id })
}
const loading = ref(false)
const selected = ref('')
const document = ref('')
const frame = ref<HTMLIFrameElement>()
let generation = 0
let pending = false

async function load() {
  loading.value = true
  failure.value = ''
  try {
    await store.refresh()
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
    document.value = `<meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data:; connect-src 'none'; form-action 'none'; base-uri 'none';">${data.html}<script>new ResizeObserver(() => parent.postMessage({type:'rs-plugin-resize',height:document.documentElement.scrollHeight},'*')).observe(document.body);<\/script>`
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
  if (message?.type === 'rs-plugin-resize' && typeof message.height === 'number' && Number.isFinite(message.height)) {
    frameHeight.value = Math.max(600, Math.min(30000, message.height))
    return
  }
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

watch(pageId, async (id) => {
  ++generation
  document.value = ''
  selected.value = ''
  frameHeight.value = 720
  await load()
  if (id !== pageId.value || !id)
    return
  const plugin = plugins.value.find(item => item.id === id)
  if (plugin?.enabled && plugin.available)
    await open(plugin)
  else failure.value = '插件已停用、启动失败或未安装。请前往插件管理检查。'
}, { immediate: true })
</script>

<template>
  <div class="flex flex-col gap-6">
    <template v-if="!pageId">
      <BasePageHeader title="插件" description="安装扩展、管理运行状态与插件页面">
        <template #actions>
          <BaseButton :disabled="loading || busy" @click="load">
            刷新
          </BaseButton>
          <BaseButton variant="primary" :disabled="busy" @click="installOpen = true">
            从 URL 安装
          </BaseButton>
        </template>
      </BasePageHeader>
      <p v-if="notice" role="status" class="text-cp-success-text">
        {{ notice }}
      </p>
      <BaseCard class="p-5">
        <p v-if="!plugins.length" class="text-cp-text-secondary">
          {{ loading ? '正在读取插件…' : '尚未安装插件。使用上方按钮粘贴插件包下载地址。' }}
        </p>
        <div v-for="plugin in plugins" :key="plugin.id" class="flex flex-wrap items-center justify-between gap-4 py-3">
          <div class="min-w-40 flex-1">
            <h2 class="font-semibold">
              {{ plugin.menuLabel || plugin.id }}
            </h2>
            <span class="text-sm text-cp-text-secondary">{{ plugin.id }} · {{ plugin.version }}</span>
          </div>
          <BaseTag :type="plugin.available ? 'success' : 'warning'">
            {{ !plugin.enabled ? '已停用' : plugin.available ? '运行中' : '启动失败' }}
          </BaseTag>
          <div class="flex w-full flex-wrap justify-end gap-2 sm:w-auto">
            <BaseButton :disabled="!plugin.available || busy" @click="router.push(`/extensions/${plugin.id}`)">
              打开
            </BaseButton>
            <BaseButton v-if="!plugin.enabled || !plugin.available" variant="primary" :disabled="busy" @click="ask(plugin, 'enable')">
              {{ plugin.enabled ? '重新启动' : '启用' }}
            </BaseButton>
            <BaseButton v-if="plugin.enabled" :disabled="busy" @click="ask(plugin, 'disable')">
              停用
            </BaseButton>
            <BaseButton variant="ghost" :disabled="plugin.enabled || busy" :title="plugin.enabled ? '请先停用插件' : '卸载并保留数据'" @click="ask(plugin, 'uninstall')">
              卸载
            </BaseButton>
          </div>
        </div>
      </BaseCard>
    </template>
    <p v-if="failure" role="alert" class="text-cp-error-text">
      {{ failure }}
    </p>
    <template v-if="pageId">
      <p v-if="loading" role="status" class="text-cp-text-secondary">
        正在打开插件页面…
      </p>
      <BaseButton v-if="failure" @click="router.push('/plugins')">
        返回插件管理
      </BaseButton>
      <iframe v-if="document" :key="generation" ref="frame" :title="`${selected} 管理页面`" :srcdoc="document" sandbox="allow-scripts" :style="{ height: `${frameHeight}px` }" class="w-full rounded-xl border-0 bg-cp-bg-container" />
    </template>
    <BaseModal v-model="installOpen" title="从 URL 安装插件" description="粘贴 tar.gz 插件包的直接下载地址。安装后处于停用状态。" :dismissible="!busy">
      <form id="install-plugin" class="flex flex-col gap-4" @submit.prevent="install">
        <FormItem label="下载地址" control-id="plugin-url" required>
          <BaseInput id="plugin-url" v-model="downloadUrl" type="url" required placeholder="https://example.com/plugin-linux-x64.tar.gz" :disabled="busy" />
        </FormItem>
        <FormItem label="SHA256（可选）" control-id="plugin-sha">
          <BaseInput id="plugin-sha" v-model="checksum" pattern="[a-fA-F0-9]{64}" placeholder="发布者提供的文件校验值" :disabled="busy" />
        </FormItem>
        <p class="text-sm text-cp-text-secondary">
          只安装可信来源的原生插件。最大 128 MB；支持 HTTP / HTTPS 直链与重定向。GitHub 仓库页面不是插件包下载地址。
        </p>
        <p v-if="busy" role="status">
          正在下载并校验，请稍候…
        </p>
        <p v-if="failure" role="alert" class="text-cp-error-text">
          {{ failure }}
        </p>
      </form>
      <template #footer>
        <BaseButton :disabled="busy" @click="installOpen = false">
          取消
        </BaseButton><BaseButton type="submit" form="install-plugin" variant="primary" :loading="busy" :disabled="!downloadUrl.trim()">
          下载安装
        </BaseButton>
      </template>
    </BaseModal>
    <BaseConfirmModal v-model="confirmOpen" :title="`${actionLabel} ${target?.menuLabel || target?.id || '插件'}`" :description="confirmation" :confirm-text="actionLabel" :destructive="action === 'uninstall'" :loading="busy" @confirm="confirm">
      <p v-if="failure" role="alert" class="text-cp-error-text">
        {{ failure }}
      </p>
    </BaseConfirmModal>
  </div>
</template>

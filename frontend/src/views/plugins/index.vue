<script setup lang="ts">
import type { PluginManagement, PluginStatus, PluginUpdateInfo } from '@/api/modules/plugins'
import { useClipboard, useEventListener } from '@vueuse/core'
import { storeToRefs } from 'pinia'
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { checkPluginUpdate, invokePlugin, managePlugin } from '@/api/modules/plugins'
import { getProxies } from '@/api/modules/proxies'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseCard from '@/components/base/BaseCard.vue'
import BaseConfirmModal from '@/components/base/BaseConfirmModal.vue'
import FormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import BaseModal from '@/components/base/BaseModal/index.vue'
import BasePageHeader from '@/components/base/BasePageHeader.vue'
import BaseSelect from '@/components/base/BaseSelect.vue'
import BaseTag from '@/components/base/BaseTag.vue'
import { usePluginsStore } from '@/stores/modules/plugins'
import { createGatewayChatBridge } from './gatewayChat'

const store = usePluginsStore()
const { plugins } = storeToRefs(store)
const route = useRoute()
const router = useRouter()
const pageId = computed(() => typeof route.params.id === 'string' ? route.params.id : '')
const busy = ref(false)
const failure = ref('')
const installOpen = ref(false)
const updateOpen = ref(false)
const downloadProxy = ref('')
const notice = ref('')
const updateTarget = ref<PluginStatus>()
const updateInfo = ref<PluginUpdateInfo>()
const updateError = ref('')
const checkingUpdate = ref(false)
const { copy } = useClipboard({ legacy: true })
let updateGeneration = 0
async function checkUpdate() {
  const plugin = updateTarget.value
  if (!plugin)
    return
  const current = ++updateGeneration
  checkingUpdate.value = true
  updateError.value = ''
  updateInfo.value = undefined
  try {
    const info = await checkPluginUpdate(plugin.id, downloadProxy.value || undefined)
    if (current === updateGeneration)
      updateInfo.value = info
  }
  catch (error) {
    if (current === updateGeneration)
      updateError.value = error instanceof Error ? error.message : '检查更新失败'
  }
  finally {
    if (current === updateGeneration)
      checkingUpdate.value = false
  }
}
function openUpdate(plugin: PluginStatus) {
  updateTarget.value = plugin
  updateOpen.value = true
  void checkUpdate()
}
watch(updateOpen, (open) => {
  if (!open) {
    ++updateGeneration
    checkingUpdate.value = false
  }
})
async function copyUpdateLink() {
  if (updateInfo.value?.downloadUrl) {
    await copy(updateInfo.value.downloadUrl)
    notice.value = '已复制新版本安装链接。更新前请先停用并卸载当前版本，数据会保留。'
    updateOpen.value = false
  }
}
const downloadUrl = ref('')
const checksum = ref('')
const proxyOptions = ref<{ label: string, value: string }[]>([{ label: '直连（不使用代理）', value: '' }])
watch([installOpen, updateOpen], async ([installing, updating]) => {
  if (!installing && !updating)
    return
  try {
    const options = [{ label: '直连（不使用代理）', value: '' }]
    for (let page = 1; page <= 20; page++) {
      const result = await getProxies({ page, pageSize: 100, search: '' }, { silent: true })
      options.push(...result.items.map(proxy => ({ label: proxy.name, value: proxy.id })))
      if (page >= result.page.totalPages)
        break
    }
    proxyOptions.value = options
  }
  catch { failure.value = '无法读取下载代理列表，可以直连重试。' }
})
const confirmOpen = ref(false)
const target = ref<PluginStatus>()
const action = ref<'enable' | 'disable' | 'uninstall'>('disable')
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
  void manage({ action: 'install', url: downloadUrl.value.trim(), sha256: checksum.value.trim() || undefined, proxyId: downloadProxy.value || undefined })
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
let gatewayChat: ReturnType<typeof createGatewayChatBridge> | undefined
onBeforeUnmount(() => gatewayChat?.dispose())

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
  gatewayChat?.dispose()
  gatewayChat = plugin.capabilities?.includes('ui.chat') ? createGatewayChatBridge(plugin.id) : undefined
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
  if (message?.type === 'rs-plugin-host-call' && typeof message.id === 'string' && message.id.length <= 64
    && ['chat.context', 'chat.start', 'chat.poll', 'chat.cancel'].includes(message.method)) {
    const current = generation
    const bridge = gatewayChat
    try {
      if (!bridge)
        throw new Error('插件没有对话页面权限')
      const data = await bridge.invoke(message.method, message.input ?? {})
      if (current === generation && target === frame.value?.contentWindow)
        target.postMessage({ type: 'rs-plugin-host-result', id: message.id, data }, '*')
    }
    catch (error) {
      if (current === generation && target === frame.value?.contentWindow)
        target.postMessage({ type: 'rs-plugin-host-result', id: message.id, error: error instanceof Error ? error.message : '操作失败' }, '*')
    }
    return
  }
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
  gatewayChat?.dispose()
  gatewayChat = undefined
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
            <div class="mt-1 flex flex-wrap gap-3 text-sm text-cp-text-secondary">
              <span>作者：{{ plugin.author || '未提供' }}</span>
              <a v-if="plugin.repository" :href="plugin.repository" target="_blank" rel="noopener noreferrer" class="text-cp-primary-text">源码仓库</a>
            </div>
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
            <BaseButton :disabled="!plugin.updateSupported || busy" :title="plugin.updateSupported ? '从发布仓库检查新版本' : '此包未声明受支持的更新源'" @click="openUpdate(plugin)">
              检查更新
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
        <FormItem label="下载代理" control-id="plugin-proxy" description="仅用于下载插件包，不改变账号或打标插件的代理设置。">
          <BaseSelect id="plugin-proxy" v-model="downloadProxy" :options="proxyOptions" :disabled="busy" />
        </FormItem>
        <FormItem label="SHA256（可选）" control-id="plugin-sha">
          <BaseInput id="plugin-sha" v-model="checksum" pattern="[a-fA-F0-9]{64}" placeholder="发布者提供的文件校验值" :disabled="busy" />
        </FormItem>
        <p class="text-sm text-cp-text-secondary">
          只安装可信来源的原生插件。最大 128 MB；支持 HTTP / HTTPS 直链与重定向。请使用 Release 附件的 tar.gz 直链，仓库主页和源码 ZIP 不是插件包。
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
    <BaseModal v-model="updateOpen" :title="`${updateTarget?.menuLabel || updateTarget?.id || '插件'} · 检查更新`">
      <div class="space-y-4">
        <p class="text-cp-text-secondary">
          当前版本：{{ updateTarget?.version }}
        </p>
        <FormItem label="访问代理" description="用于访问 GitHub 发布接口。">
          <BaseSelect v-model="downloadProxy" :options="proxyOptions" :disabled="checkingUpdate" />
        </FormItem>
        <p v-if="checkingUpdate" role="status">
          正在检查更新…
        </p>
        <p v-if="updateError" role="alert" class="text-cp-error-text">
          {{ updateError }}
        </p>
        <template v-if="updateInfo">
          <p role="status">
            {{ updateInfo.updateAvailable ? `发现新版本 ${updateInfo.latestVersion}` : updateInfo.latestVersion ? '当前已是最新可用版本' : '仓库暂未提供匹配的发布包' }}{{ updateInfo.prerelease ? '（预发布）' : '' }}
          </p>
          <a v-if="updateInfo.releaseUrl" :href="updateInfo.releaseUrl" target="_blank" rel="noopener noreferrer" class="text-cp-primary-text">查看发布说明</a>
          <p v-if="updateInfo.updateAvailable" class="text-sm text-cp-text-secondary">
            复制安装链接后，停用并卸载当前版本，再安装新包。现有插件数据会保留。
          </p>
          <p v-if="updateInfo.sha256" class="break-all text-xs text-cp-text-secondary">
            SHA256：{{ updateInfo.sha256 }}
          </p>
        </template>
      </div>
      <template #footer>
        <BaseButton :disabled="checkingUpdate" @click="checkUpdate">
          重新检查
        </BaseButton>
        <BaseButton v-if="updateInfo?.updateAvailable && updateInfo.downloadUrl" variant="primary" @click="copyUpdateLink">
          复制安装链接
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

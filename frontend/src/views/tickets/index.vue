<script setup lang="ts">
import type { OutboundProxyRecord } from '@/api/modules/proxies'
import type { TicketAccount, TicketMode, TicketPanel, TicketPolicy, TicketProxyInput, TicketSettings } from '@/api/modules/tickets'
import { ChevronLeft, ChevronRight, Plus, Trash2 } from '@lucide/vue'
import { useIntervalFn } from '@vueuse/core'
import { computed, onMounted, ref, toRaw, watch } from 'vue'
import { getProxies } from '@/api/modules/proxies'
import { continuousTicket, getTicketPanel, probeTicket, saveTicketSettings } from '@/api/modules/tickets'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseCard from '@/components/base/BaseCard.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import BasePageHeader from '@/components/base/BasePageHeader.vue'
import BaseSelect from '@/components/base/BaseSelect.vue'
import BaseSwitch from '@/components/base/BaseSwitch.vue'
import { toast } from '@/components/base/BaseToast'
import { useUiClock } from '@/composables/useUiClock'

const panel = ref<TicketPanel | null>(null)
const draft = ref<TicketSettings | null>(null)
const draftRevision = ref(0)
let generation = 0
let polling = false
const loading = ref(false)
const saving = ref(false)
const probing = ref('')
const controlling = ref('')
const manualProxies = ref<Record<string, string>>({})
const continuousIntervals = ref<Record<string, string>>({})
const manualProxyOptions = computed(() => [
  { label: '代理池轮换', value: 'pool' },
  ...(panel.value?.proxies ?? []).map(p => ({ label: `${p.name}${p.enabled ? '' : '（已禁用）'}`, value: p.id, disabled: !p.enabled })),
])
watch(() => panel.value?.revision, () => {
  manualProxies.value = {}
})
const proxy = ref('')
const clearProxy = ref(false)
const poolDraft = ref<(TicketProxyInput & { key: number, endpoint: string, hasAuthentication: boolean })[]>([])
let proxyKey = 0
const savedProxies = ref<OutboundProxyRecord[]>([])
const selectedProxy = ref('')
const importing = ref(false)
const savedProxyOptions = computed(() => savedProxies.value.map(p => ({ value: p.id, label: `${p.name} · ${p.endpoint}` })))
function addProxy() {
  poolDraft.value.push({ key: ++proxyKey, name: `代理 ${poolDraft.value.length + 1}`, url: '', endpoint: '', hasAuthentication: false, enabled: true, concurrency: 1 })
}
async function loadSavedProxies() {
  importing.value = true
  try {
    const items: OutboundProxyRecord[] = []
    for (let page = 1; ; page++) {
      const result = await getProxies({ page, pageSize: 100 })
      items.push(...result.items)
      if (page >= result.page.totalPages)
        break
    }
    savedProxies.value = items
    if (!items.length)
      toast.warning('RS 代理管理中暂无已保存代理')
  }
  catch {}
  finally { importing.value = false }
}
function importProxy() {
  const item = savedProxies.value.find(p => p.id === selectedProxy.value)
  if (!item)
    return
  poolDraft.value.push({ key: ++proxyKey, name: item.name, savedProxyId: item.id, endpoint: item.endpoint, hasAuthentication: item.hasAuthentication, enabled: true, concurrency: 1 })
  selectedProxy.value = ''
}
const search = ref('')
const logSearch = ref('')
const logOutcome = ref('all')
const logPage = ref(1)
const logOptions = [{ label: '全部结果', value: 'all' }, { label: '已命中', value: 'matched' }, { label: '未命中 / 失败', value: 'failed' }]
const filteredLogs = computed(() => (panel.value?.logs ?? []).filter((log) => {
  const text = `${log.accountName} ${log.result.accountId} ${log.result.model} ${log.proxyName ?? ''} ${log.proxyEndpoint} ${log.result.httpStatus} ${log.result.message}`.toLowerCase()
  return text.includes(logSearch.value.toLowerCase()) && (logOutcome.value === 'all' || log.result.matched === (logOutcome.value === 'matched'))
}))
const logPages = computed(() => Math.max(1, Math.ceil(filteredLogs.value.length / 25)))
const visibleLogs = computed(() => filteredLogs.value.slice((logPage.value - 1) * 25, logPage.value * 25))
const proxyStats = computed(() => {
  const stats = new Map<string, { endpoint: string, count: number, matched: number }>()
  for (const log of panel.value?.logs ?? []) {
    const item = stats.get(log.proxyEndpoint) ?? { endpoint: log.proxyEndpoint, count: 0, matched: 0 }
    item.count++
    item.matched += Number(log.result.matched)
    stats.set(item.endpoint, item)
  }
  return [...stats.values()].sort((a, b) => b.matched - a.matched || b.count - a.count)
})
watch([logSearch, logOutcome], () => {
  logPage.value = 1
})
watch(logPages, (pages) => {
  logPage.value = Math.min(logPage.value, pages)
})
const failure = ref('')
const models = ref('')
const policies = ref<Record<string, TicketPolicy>>({})
const customLengths = ref<Record<string, string>>({})
const clock = useUiClock()
const modeOptions = [
  { label: '关闭打标', value: 'off' },
  { label: '仅手动', value: 'manual' },
  { label: '自动打标', value: 'auto' },
]
const rows = computed(() => (panel.value?.accounts ?? []).filter(a => `${a.name} ${a.id} ${a.plan ?? ''}`.toLowerCase().includes(search.value.toLowerCase())))
const readyCount = computed(() => panel.value?.accounts.reduce((n, a) => n + a.models.filter(m => m.ready).length, 0) ?? 0)

function accept(data: TicketPanel) {
  panel.value = data
  draftRevision.value = data.revision
  draft.value = structuredClone(data.settings)
  draft.value.manualIntervalSeconds ??= 0
  draft.value.proxyPoolEnabled ??= true
  models.value = data.settings.models.join(', ')
  policies.value = Object.fromEntries(data.accounts.map(a => [a.id, { ...a.policy }]))
  customLengths.value = Object.fromEntries(data.accounts.map(a => [a.id, a.policy.targetLength?.toString() ?? '']))
  proxy.value = ''
  poolDraft.value = (data.proxies ?? []).map(p => ({ ...p, key: ++proxyKey, url: '', enabled: p.enabled ?? true, concurrency: p.concurrency ?? 1 }))
  clearProxy.value = false
}
async function load() {
  generation += 1
  loading.value = true
  failure.value = ''
  try {
    accept(await getTicketPanel())
  }
  catch { failure.value = '读取打标状态失败，请重试。' }
  finally { loading.value = false }
}
function setMode(id: string, value: string) {
  const old = policies.value[id]
  policies.value[id] = { mode: value as TicketMode, targetLength: old?.targetLength ?? null }
}
async function save() {
  if (!draft.value || !panel.value)
    return
  const settings = structuredClone(toRaw(draft.value))
  settings.models = models.value.split(/[,，\s]+/).filter(Boolean)
  settings.accounts = Object.fromEntries(Object.entries(policies.value).map(([id, policy]) => [id, {
    ...policy,
    targetLength: customLengths.value[id]?.trim() ? Number(customLengths.value[id]) : null,
  }]))
  const lengths = [settings.plusProLength, settings.businessLength, settings.defaultLength, ...Object.values(settings.accounts).flatMap(p => p.targetLength === null ? [] : [p.targetLength])]
  if (lengths.some(n => !Number.isInteger(n) || n < 64 || n > 4096)) {
    toast.warning('目标长度应为 64–4096 的整数')
    return
  }
  const ranges: [number, number, number, string][] = [
    [settings.intervalSeconds, 10, 86400, '自动探测间隔应为 10–86400 秒的整数'],
    [settings.manualIntervalSeconds, 0, 86400, '手动探测间隔应为 0–86400 秒的整数'],
    [settings.ttlSeconds, 60, 3600, '有效期应为 60–3600 秒的整数'],
    [settings.refreshBeforeSeconds, 0, settings.ttlSeconds - 1, '提前刷新时间必须为非负整数且小于有效期'],
  ]
  for (const [value, min, max, message] of ranges) {
    if (!Number.isInteger(value) || value < min || value > max) {
      toast.warning(message)
      return
    }
  }
  if (!settings.models.length || settings.models.length > 8 || new Set(settings.models).size !== settings.models.length || settings.models.some(model => !/^[\w.-]{1,128}$/.test(model))) {
    toast.warning('请填写 1–8 个不重复的有效模型名称')
    return
  }
  const proxyPool = proxy.value.split(/\r?\n/).map(value => value.trim()).filter(Boolean)
  const proxies: TicketProxyInput[] = poolDraft.value.map(p => ({ id: p.id, name: p.name.trim(), url: p.url?.trim() || undefined, savedProxyId: p.savedProxyId, enabled: p.enabled ?? true, concurrency: p.concurrency ?? 1 }))
  proxies.push(...proxyPool.map((url, i) => ({ name: `代理 ${poolDraft.value.length + i + 1}`, url })))
  if (!clearProxy.value && proxies.length > 64) {
    toast.warning('代理池最多 64 个代理')
    return
  }
  if (!clearProxy.value && proxies.some(p => !p.name || (!p.id && !p.savedProxyId && !p.url))) {
    toast.warning('请填写代理名称和新代理完整地址')
    return
  }
  if (proxies.some(p => p.concurrency !== undefined && (!Number.isInteger(p.concurrency) || p.concurrency < 1 || p.concurrency > 3))) {
    toast.warning('每个代理入口并发应为 1–3')
    return
  }
  if (settings.enabled && (clearProxy.value || !proxies.length)) {
    toast.warning('启用打标需要配置代理池；清除代理池前请关闭打标')
    return
  }
  saving.value = true
  generation += 1
  try {
    accept(await saveTicketSettings({ revision: draftRevision.value, settings, proxies: clearProxy.value ? [] : proxies }))
    toast.success('策略已保存，仍符合规则的有效票已保留')
  }
  catch {}
  finally { saving.value = false }
}
async function probe(account: TicketAccount, model: string) {
  generation += 1
  probing.value = `${account.id}/${model}`
  try {
    const result = await probeTicket(manualInput(account.id, model))
    if (result.matched)
      toast.success(`${account.name}：已匹配，长度 ${result.length}`)
    else toast.warning(`${account.name}：HTTP ${result.httpStatus}，长度 ${result.length}，${result.message}`)
    // 仅更新状态，保留尚未保存的策略草稿。
    panel.value = await getTicketPanel()
  }
  catch {}
  finally { probing.value = '' }
}
function time(value: number | null | undefined) {
  return value ? new Date(value * 1000).toLocaleString() : '—'
}
function manualInput(accountId: string, model: string) {
  const proxyId = manualProxies.value[`${accountId}/${model}`] ?? 'pool'
  return { accountId, model, proxyId: proxyId === 'pool' ? undefined : proxyId, revision: panel.value?.revision }
}
async function continuous(account: TicketAccount, model: string, stop = false) {
  const key = `${account.id}/${model}`
  const interval = Number(continuousIntervals.value[key] ?? '10')
  if (!stop && (!Number.isInteger(interval) || interval < 10 || interval > 86400)) {
    toast.warning('持续打标间隔应为 10–86400 秒')
    return
  }
  generation += 1
  controlling.value = key
  try {
    panel.value = await continuousTicket({ ...manualInput(account.id, model), intervalSeconds: stop ? null : interval })
    toast.success(stop ? '持续打标已停止' : '已提交持续打标，命中后自动停止')
  }
  catch {}
  finally { controlling.value = '' }
}
onMounted(load)
useIntervalFn(async () => {
  if (loading.value || saving.value || probing.value || controlling.value || !panel.value || polling || document.hidden)
    return
  polling = true
  const started = generation
  try {
    const data = await getTicketPanel({ silent: true })
    if (started === generation)
      panel.value = data
  }
  catch {}
  finally { polling = false }
}, computed(() => panel.value?.accounts.some(a => a.models.some(m => m.continuous)) ? 3000 : 10000))
</script>

<template>
  <div class="flex flex-col gap-6">
    <BasePageHeader title="打标管理" description="按账号管理 Turn-State 探测与复用">
      <template #actions>
        <BaseButton :disabled="loading || saving || !!probing" @click="load">
          刷新 / 重置草稿
        </BaseButton>
        <BaseButton variant="primary" :disabled="!draft || saving || !!probing" @click="save">
          {{ saving ? '保存中…' : '保存策略' }}
        </BaseButton>
      </template>
    </BasePageHeader>
    <p v-if="failure" role="alert" class="text-cp-error-text">
      {{ failure }}
    </p>
    <p v-if="loading && !panel" role="status" class="text-cp-text-secondary">
      正在读取账号与票状态…
    </p>
    <template v-if="panel && draft">
      <p v-if="draftRevision !== panel.revision" role="status" class="text-cp-warning-text">
        策略已在其他页面更新，请刷新后再编辑保存。
      </p>
      <BaseCard class="p-5">
        <div class="flex flex-wrap items-center justify-between gap-4">
          <div>
            <h2 class="text-xl font-semibold">
              运行策略
            </h2><p class="mt-1 text-cp-text-secondary">
              {{ panel.accounts.length }} 个 OAuth 账号 · {{ readyCount }} 张有效票
            </p>
          </div>
          <div class="flex flex-wrap gap-5">
            <BaseSwitch v-model="draft.enabled" label="启用打标" show-label />
            <BaseSwitch v-model="draft.inject" label="业务请求使用有效票" show-label />
          </div>
        </div>
        <p class="mt-4 text-sm text-cp-text-secondary">
          长度是可配置的匹配规则，不代表模型能力或质量。未获票时维持原转发；关闭打标的账号不探测、不注入。保存会保留仍符合规则的有效票。
        </p>
        <div class="mt-5 grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
          <label class="flex flex-col gap-2" for="ticket-plus">Plus / Pro 默认长度<input id="ticket-plus" v-model.number="draft.plusProLength" type="number" min="64" max="4096" class="rounded-lg bg-cp-fill-tertiary p-3"></label>
          <label class="flex flex-col gap-2" for="ticket-business">Business / Team 默认长度<input id="ticket-business" v-model.number="draft.businessLength" type="number" min="64" max="4096" class="rounded-lg bg-cp-fill-tertiary p-3"></label>
          <label class="flex flex-col gap-2" for="ticket-default">其他套餐默认长度<input id="ticket-default" v-model.number="draft.defaultLength" type="number" min="64" max="4096" class="rounded-lg bg-cp-fill-tertiary p-3"></label>
          <label class="flex flex-col gap-2" for="ticket-interval">自动探测间隔（秒）<input id="ticket-interval" v-model.number="draft.intervalSeconds" type="number" min="10" max="86400" step="1" class="rounded-lg bg-cp-fill-tertiary p-3"></label>
          <label class="flex flex-col gap-2" for="ticket-manual-interval">手动探测间隔（秒，0 为无等待）<input id="ticket-manual-interval" v-model.number="draft.manualIntervalSeconds" type="number" min="0" max="86400" step="1" class="rounded-lg bg-cp-fill-tertiary p-3"></label>
          <label class="flex flex-col gap-2" for="ticket-ttl">有效期（秒，最多 3600）<input id="ticket-ttl" v-model.number="draft.ttlSeconds" type="number" min="60" max="3600" class="rounded-lg bg-cp-fill-tertiary p-3"></label>
          <label class="flex flex-col gap-2" for="ticket-refresh">提前刷新（秒）<input id="ticket-refresh" v-model.number="draft.refreshBeforeSeconds" type="number" min="0" :max="draft.ttlSeconds - 1" class="rounded-lg bg-cp-fill-tertiary p-3"></label>
        </div>
        <div class="mt-5 grid gap-4 lg:grid-cols-2">
          <div class="flex flex-col gap-2">
            <span>目标模型（逗号分隔）</span><BaseInput id="ticket-models" v-model="models" aria-label="目标模型（逗号分隔）" />
          </div>
          <label class="flex flex-col gap-2" for="ticket-proxy">
            <span>批量追加代理</span><textarea id="ticket-proxy" v-model="proxy" aria-label="批量追加代理" autocomplete="off" spellcheck="false" placeholder="每行一个完整代理地址" class="min-h-24 rounded-lg bg-cp-fill-tertiary p-3 font-mono text-sm" :disabled="clearProxy" />
          </label>
        </div>
        <div class="mt-4 flex flex-wrap items-center gap-3">
          <h3 class="font-semibold">
            已保存 {{ panel.proxyCount }} 个代理
          </h3>
          <BaseSwitch v-model="draft.proxyPoolEnabled" label="启用打标代理池" show-label />
          <BaseButton :disabled="clearProxy || poolDraft.length >= 64" title="新增代理" aria-label="新增代理" @click="addProxy">
            <Plus :size="16" />
          </BaseButton>
          <BaseButton :disabled="importing" @click="loadSavedProxies">
            {{ importing ? '读取中…' : '读取 RS 已有代理' }}
          </BaseButton>
          <BaseSelect v-if="savedProxies.length" v-model="selectedProxy" :options="savedProxyOptions" aria-label="选择已有代理" class="w-full sm:w-80" />
          <BaseButton v-if="savedProxies.length" :disabled="!selectedProxy || clearProxy" @click="importProxy">
            加入打标池
          </BaseButton>
        </div>
        <div v-for="entry in poolDraft" :key="entry.key" class="mt-3 flex flex-wrap items-center gap-3">
          <BaseSwitch v-model="entry.enabled" :label="`启用 ${entry.name || '代理'}`" :disabled="clearProxy" />
          <BaseInput v-model="entry.name" aria-label="代理名称" class="w-full sm:w-44" :disabled="clearProxy" />
          <span class="min-w-0 flex-1 break-all font-mono text-sm">{{ entry.endpoint || '新代理' }} · {{ entry.hasAuthentication ? '已保存认证' : '无已保存认证' }}</span>
          <BaseInput v-if="!entry.savedProxyId" v-model="entry.url" :aria-label="`${entry.name} 代理地址`" type="password" autocomplete="new-password" :placeholder="entry.id !== undefined ? '留空保留地址及认证；输入完整 URL 替换' : 'socks5://用户名:密码@主机:端口'" class="w-full sm:w-80" :disabled="clearProxy" />
          <span v-else class="text-sm text-cp-text-secondary">保存时导入认证</span>
          <BaseSelect :model-value="String(entry.concurrency ?? 1)" :options="[{ label: '并发 1', value: '1' }, { label: '并发 2', value: '2' }, { label: '并发 3', value: '3' }]" :aria-label="`${entry.name} 并发数`" :disabled="clearProxy" class="w-28" @update:model-value="entry.concurrency = Number($event)" />
          <BaseButton :aria-label="`移除 ${entry.name}`" title="移除代理" :disabled="clearProxy" @click="poolDraft = poolDraft.filter(p => p.key !== entry.key)">
            <Trash2 :size="16" />
          </BaseButton>
        </div>
        <div class="mt-4">
          <p class="mb-3 text-sm text-cp-text-secondary">
            暂停代理池或禁用单个入口会保留配置和已获有效票，已发出的探测会正常结束。并发按代理入口计算，整个服务最多同时 12 个探测。
          </p>
          <BaseSwitch v-model="clearProxy" label="清除已保存的代理（需同时关闭打标）" show-label />
        </div>
      </BaseCard>
      <BaseCard class="p-5">
        <div class="mb-5 flex flex-wrap items-center justify-between gap-4">
          <h2 class="text-xl font-semibold">
            账号与手动探测
          </h2>
          <BaseInput v-model="search" aria-label="搜索账号" placeholder="搜索账号、套餐或 ID" class="w-full sm:w-72" />
        </div>
        <p class="mb-4 text-sm text-cp-text-secondary">
          修改模式后先保存。手动探测可指定代理或从池内轮换；持续打标按间隔重复，命中即停，也可随时停止。限流会退避。刷新会放弃未保存草稿，持续任务继续运行。
          后台最近检查：{{ time(panel.workerCheckedAt) }}。
        </p>
        <p v-if="!rows.length" class="py-8 text-center text-cp-text-secondary">
          暂无匹配的 OAuth 账号
        </p>
        <div v-else class="flex flex-col gap-4">
          <article v-for="account in rows" :key="account.id" class="rounded-xl bg-cp-fill-quaternary p-4">
            <div class="flex flex-wrap items-start justify-between gap-4">
              <div class="min-w-0">
                <h3 class="break-all font-semibold">
                  {{ account.name }}
                </h3><p class="mt-1 text-sm text-cp-text-secondary">
                  {{ account.plan || '未知套餐' }} · 已保存目标 {{ account.targetLength }} · {{ account.eligible ? '可探测' : '账号当前不可探测' }}
                </p>
              </div>
              <div class="flex flex-wrap gap-3">
                <BaseSelect :model-value="policies[account.id]?.mode ?? 'off'" :options="modeOptions" :aria-label="`${account.name} 打标模式`" class="w-36" @update:model-value="setMode(account.id, $event)" />
                <BaseInput v-model="customLengths[account.id]" :aria-label="`${account.name} 自定义长度`" placeholder="留空跟随套餐" type="number" min="64" max="4096" class="w-40" />
              </div>
            </div>
            <div class="mt-4 grid gap-3 lg:grid-cols-2">
              <div v-for="status in account.models" :key="status.model" class="rounded-lg bg-cp-bg-container p-4">
                <div class="flex flex-wrap items-center justify-between gap-2">
                  <span class="font-mono text-sm">{{ status.model }}</span><span :class="status.ready ? 'text-cp-success-text' : 'text-cp-text-secondary'">{{ status.ready ? '有效' : '暂无有效票' }}</span>
                </div>
                <p class="mt-2 text-sm text-cp-text-secondary">
                  最近：{{ status.lastResult ? `HTTP ${status.lastResult.httpStatus} / 长度 ${status.lastResult.length}` : '尚未探测' }}
                </p>
                <p class="mt-1 text-xs text-cp-text-secondary">
                  到期：{{ time(status.expiresAt) }}<span v-if="status.manualRetryAt"> · 下次可手动探测：{{ time(status.manualRetryAt) }}</span>
                </p>
                <div class="mt-3 flex flex-wrap gap-3">
                  <BaseSelect :model-value="status.continuous ? (status.continuous.proxyId ?? 'pool') : (manualProxies[`${account.id}/${status.model}`] ?? 'pool')" :options="manualProxyOptions" :disabled="!!status.continuous || status.busy" :aria-label="`${account.name} ${status.model} 手动代理`" class="min-w-40 flex-1" @update:model-value="manualProxies[`${account.id}/${status.model}`] = $event" />
                  <BaseInput :model-value="status.continuous?.intervalSeconds.toString() ?? continuousIntervals[`${account.id}/${status.model}`] ?? '10'" :disabled="!!status.continuous" :aria-label="`${account.name} ${status.model} 持续间隔（秒）`" type="number" min="10" max="86400" class="w-28" @update:model-value="continuousIntervals[`${account.id}/${status.model}`] = String($event)" />
                </div>
                <p class="mt-1 text-xs text-cp-text-secondary">
                  持续间隔（秒），默认 10；每次完成后等待，命中自动停止。
                </p>
                <div class="mt-3 flex flex-wrap gap-2">
                  <BaseButton :disabled="!!probing || saving || !!controlling || !!status.continuous || !panel.settings.enabled || !panel.settings.proxyPoolEnabled || !panel.proxies.some(p => p.enabled) || !account.eligible || account.policy.mode === 'off' || status.busy || !!(status.manualRetryAt && status.manualRetryAt > clock.getTime() / 1000)" @click="probe(account, status.model)">
                    {{ probing === `${account.id}/${status.model}` || status.busy ? '探测中…' : '手动打一张' }}
                  </BaseButton>
                  <BaseButton v-if="status.continuous" :disabled="!!controlling" @click="continuous(account, status.model, true)">
                    {{ controlling === `${account.id}/${status.model}` ? '停止中…' : '停止持续打标' }}
                  </BaseButton>
                  <BaseButton v-else variant="primary" :disabled="!!probing || saving || !!controlling || !panel.settings.enabled || !panel.settings.proxyPoolEnabled || !panel.proxies.some(p => p.enabled) || !account.eligible || account.policy.mode === 'off' || status.busy || status.ready" @click="continuous(account, status.model)">
                    持续打标
                  </BaseButton>
                </div>
                <p v-if="status.continuous" role="status" class="mt-2 text-xs text-cp-text-secondary">
                  {{ status.busy ? '持续打标正在探测…' : `持续打标等待中 · 下次检查 ${time(status.continuous.nextProbeAt)}` }}
                </p>
              </div>
            </div>
          </article>
        </div>
      </BaseCard>
      <section aria-labelledby="ticket-log-title" class="min-w-0">
        <div class="flex flex-wrap items-center justify-between gap-3">
          <h2 id="ticket-log-title" class="text-xl font-semibold">
            打标日志
          </h2>
          <span class="text-sm text-cp-text-secondary">最近 {{ panel.logs?.length ?? 0 }} / {{ panel.logLimit ?? 1000 }} 条</span>
        </div>
        <p class="mt-2 text-sm text-cp-text-secondary">
          代理地址不等于真实出口 IP；动态代理的出口 IP 未确认。日志从本次升级后开始记录。
        </p>
        <div v-if="proxyStats.length" class="mt-4 overflow-x-auto">
          <table class="w-full text-left text-sm">
            <thead>
              <tr>
                <th class="p-2">
                  代理地址
                </th><th class="p-2">
                  次数
                </th><th class="p-2">
                  命中
                </th><th class="p-2">
                  命中率
                </th>
              </tr>
            </thead>
            <tbody>
              <tr v-for="item in proxyStats" :key="item.endpoint">
                <td class="break-all p-2 font-mono">
                  {{ item.endpoint }}
                </td><td class="p-2">
                  {{ item.count }}
                </td><td class="p-2">
                  {{ item.matched }}
                </td><td class="p-2">
                  {{ (100 * item.matched / item.count).toFixed(1) }}%
                </td>
              </tr>
            </tbody>
          </table>
        </div>
        <div class="my-4 flex flex-wrap gap-3">
          <BaseInput v-model="logSearch" aria-label="搜索打标日志" placeholder="账号、模型、代理 IP 或状态" class="w-full sm:w-80" />
          <BaseSelect v-model="logOutcome" :options="logOptions" aria-label="打标日志结果筛选" class="w-44" />
        </div>
        <p v-if="!visibleLogs.length" class="py-6 text-cp-text-secondary">
          暂无匹配的打标记录
        </p>
        <div v-else class="overflow-x-auto">
          <table class="w-full min-w-[960px] text-left text-sm">
            <thead>
              <tr>
                <th class="p-3">
                  时间 / 触发
                </th><th class="p-3">
                  账号 / 模型
                </th><th class="p-3">
                  代理地址
                </th><th class="p-3">
                  HTTP / 长度
                </th><th class="p-3">
                  结果 / 耗时
                </th><th class="p-3">
                  详情
                </th>
              </tr>
            </thead>
            <tbody>
              <tr v-for="log in visibleLogs" :key="log.id" class="odd:bg-cp-fill-quaternary">
                <td class="p-3 align-top">
                  {{ time(log.startedAt) }}<div class="mt-1 text-cp-text-secondary">
                    {{ log.trigger === 'auto' ? '自动' : '手动' }}
                  </div>
                </td>
                <td class="max-w-56 break-all p-3 align-top">
                  {{ log.accountName }}<div class="mt-1 font-mono text-xs">
                    {{ log.result.accountId }}
                  </div><div class="mt-1 font-mono text-xs">
                    {{ log.result.model }}
                  </div>
                </td>
                <td class="max-w-64 break-all p-3 align-top font-mono text-xs">
                  <div v-if="log.proxyName" class="mb-1 font-sans">
                    {{ log.proxyName }}
                  </div>
                  {{ log.proxyEndpoint }}
                </td>
                <td class="p-3 align-top">
                  {{ log.result.httpStatus || '无响应' }}<div class="mt-1">
                    {{ log.result.length }} / {{ log.targetLength }}
                  </div>
                </td>
                <td class="p-3 align-top">
                  <span :class="log.result.matched ? 'text-cp-success-text' : 'text-cp-warning-text'">{{ log.result.matched ? '已命中' : '未命中' }}</span><div class="mt-1">
                    {{ log.durationMs }} ms
                  </div>
                </td>
                <td class="max-w-80 break-words p-3 align-top">
                  {{ log.result.message }}<div v-if="log.retryAt" class="mt-1 text-xs">
                    自动退避至 {{ time(log.retryAt) }}
                  </div><div class="mt-1 break-all font-mono text-xs text-cp-text-tertiary">
                    {{ log.id }}
                  </div>
                </td>
              </tr>
            </tbody>
          </table>
        </div>
        <div class="mt-4 flex items-center justify-end gap-3">
          <span class="text-sm text-cp-text-secondary">{{ filteredLogs.length }} 条 · {{ logPage }} / {{ logPages }}</span>
          <BaseButton aria-label="上一页日志" title="上一页日志" :disabled="logPage <= 1" @click="logPage--">
            <ChevronLeft :size="16" />
          </BaseButton>
          <BaseButton aria-label="下一页日志" title="下一页日志" :disabled="logPage >= logPages" @click="logPage++">
            <ChevronRight :size="16" />
          </BaseButton>
        </div>
      </section>
    </template>
  </div>
</template>

<script setup lang="ts">
import type { TicketAccount, TicketMode, TicketPanel, TicketPolicy, TicketSettings } from '@/api/modules/tickets'
import { useIntervalFn } from '@vueuse/core'
import { computed, onMounted, ref, toRaw } from 'vue'
import { getTicketPanel, probeTicket, saveTicketSettings } from '@/api/modules/tickets'
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
const proxy = ref('')
const clearProxy = ref(false)
const search = ref('')
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
  models.value = data.settings.models.join(', ')
  policies.value = Object.fromEntries(data.accounts.map(a => [a.id, { ...a.policy }]))
  customLengths.value = Object.fromEntries(data.accounts.map(a => [a.id, a.policy.targetLength?.toString() ?? '']))
  proxy.value = ''
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
  if (!clearProxy.value && proxyPool.length > 64) {
    toast.warning('代理池最多 64 个代理')
    return
  }
  if (settings.enabled && (clearProxy.value || (!proxyPool.length && !panel.value.proxyConfigured))) {
    toast.warning('启用打标需要配置代理池；清除代理池前请关闭打标')
    return
  }
  saving.value = true
  generation += 1
  try {
    accept(await saveTicketSettings({ revision: draftRevision.value, settings, proxyPool: clearProxy.value ? [] : proxyPool.length ? proxyPool : undefined }))
    toast.success('策略已保存，旧票已失效；后续按新规则打标')
  }
  catch {}
  finally { saving.value = false }
}
async function probe(account: TicketAccount, model: string) {
  generation += 1
  probing.value = `${account.id}/${model}`
  try {
    const result = await probeTicket({ accountId: account.id, model })
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
onMounted(load)
useIntervalFn(async () => {
  if (loading.value || saving.value || probing.value || !panel.value || polling)
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
}, 10000)
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
          长度是可配置的匹配规则，不代表模型能力或质量。未获票时维持原转发；关闭打标的账号不探测、不注入。保存策略会清除旧票。
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
            <span>专用打标代理池 · {{ panel.proxyConfigured ? `已配置 ${panel.proxyCount ?? 1} 个，留空保留` : '尚未配置' }}</span><textarea id="ticket-proxy" v-model="proxy" aria-label="专用打标代理池" autocomplete="off" spellcheck="false" placeholder="每行一个代理" class="min-h-24 rounded-lg bg-cp-fill-tertiary p-3 font-mono text-sm" :disabled="clearProxy" />
          </label>
        </div>
        <div class="mt-4">
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
          修改模式后先保存。自动模式也支持手动打一张；每次按钮只发送一个请求，限流会退避。刷新会放弃未保存草稿。
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
                <BaseButton class="mt-3" :disabled="!!probing || saving || !panel.settings.enabled || !panel.proxyConfigured || !account.eligible || account.policy.mode === 'off' || status.busy || !!(status.manualRetryAt && status.manualRetryAt > clock.getTime() / 1000)" @click="probe(account, status.model)">
                  {{ probing === `${account.id}/${status.model}` || status.busy ? '探测中…' : '手动打一张' }}
                </BaseButton>
              </div>
            </div>
          </article>
        </div>
      </BaseCard>
    </template>
  </div>
</template>

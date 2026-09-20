import { API_BASE_URL } from '@/api/constants'
import { getApiKeys, revealApiKey } from '@/api/modules/api-keys'
import { getPlugins } from '@/api/modules/plugins'
import { renderChatMarkdown } from './chatMarkdown'

interface Message { role: 'user' | 'assistant', content: string }
interface JobResult {
  state: 'running' | 'completed' | 'failed' | 'cancelled'
  text: string
  error?: string
  models?: string[]
  httpStatus?: number
  requestId?: string
  elapsedMs?: number
  usage?: Record<string, number>
}
interface Job { result: JobResult, controller: AbortController, renderedText?: string, renderedHtml?: string }

function record(value: unknown): Record<string, unknown> {
  return value && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : {}
}

// 请求地址和 Key 明文只由宿主决定；插件只能提交选定 Key 的 ID 和有界对话内容。
export function createGatewayChatBridge(pluginId: string) {
  const jobs = new Map<string, Job>()
  let disposed = false
  let sequence = 0

  async function permission() {
    if (disposed)
      throw new Error('插件页面已关闭')
    const plugin = (await getPlugins()).find(item => item.id === pluginId)
    if (disposed || !plugin?.enabled || !plugin.available || !plugin.capabilities?.includes('ui.chat'))
      throw new Error('插件未启用或没有对话页面权限')
  }

  async function keys() {
    const result: { id: string, name: string, prefix: string }[] = []
    let cursor: string | undefined
    for (let page = 0; page < 20; page++) {
      const data = await getApiKeys({ limit: 100, cursor }, { silent: true })
      result.push(...data.items.filter(key => key.enabled).map(key => ({ id: key.id, name: key.name, prefix: key.prefix })))
      if (!data.nextCursor)
        return result
      cursor = data.nextCursor
    }
    throw new Error('Key 数量过多，暂不支持在插件中加载完整列表')
  }

  function dispose() {
    disposed = true
    for (const job of jobs.values())
      job.controller.abort()
    jobs.clear()
  }

  async function run(job: Job, input: Record<string, unknown>) {
    const start = performance.now()
    const timeout = window.setTimeout(() => job.controller.abort(), 120000)
    let secret = ''
    const clean = (text: string) => secret ? text.replaceAll(secret, '[已隐藏]') : text
    const checkCancelled = () => {
      if (disposed || job.controller.signal.aborted)
        throw new Error('请求已取消或超时')
    }
    try {
      await permission()
      if (!(await keys()).some(key => key.id === input.keyId))
        throw new Error('所选 Key 已停用或不存在，请刷新 Key 列表')
      checkCancelled()
      secret = (await revealApiKey({ id: input.keyId as string }, { silent: true, signal: job.controller.signal })).plaintextKey
      checkCancelled()
      // 模型与对话只允许当前浏览器同源的标准端点，拒绝重定向，避免把凭据带到外部站点。
      const models = input.kind === 'models'
      const url = new URL(`${API_BASE_URL}/v1/${models ? 'models' : 'responses'}`, window.location.origin)
      const messages = input.messages as Message[] | undefined
      const response = await fetch(url, {
        method: models ? 'GET' : 'POST',
        headers: { 'Authorization': `Bearer ${secret}`, 'Content-Type': 'application/json' },
        credentials: 'omit',
        redirect: 'error',
        signal: job.controller.signal,
        body: models
          ? undefined
          : JSON.stringify({
              model: input.model,
              input: messages?.map(message => ({ role: message.role, content: [{ type: message.role === 'assistant' ? 'output_text' : 'input_text', text: message.content }] })),
              instructions: 'You are a helpful assistant.',
              stream: true,
              store: false,
            }),
      })
      job.result.httpStatus = response.status
      job.result.requestId = clean(response.headers.get('x-gateway-request-id') || response.headers.get('x-request-id') || '').slice(0, 128)
      if (!response.body)
        throw new Error('RS 没有返回响应正文')
      const reader = response.body.getReader()
      const decoder = new TextDecoder('utf-8', { fatal: true })
      const sse = response.ok && response.headers.get('content-type')?.includes('text/event-stream')
      let bytes = 0
      let buffer = ''
      let completed = false
      let streamingText = ''
      const streamText = (text: string) => {
        const safe = clean(text)
        // 暂缓输出仍可能构成 Key 的末尾片段，防止跨 SSE 分片反射凭据。
        for (let length = Math.min(secret.length - 1, safe.length); length > 0; length--) {
          if (safe.endsWith(secret.slice(0, length)))
            return safe.slice(0, -length)
        }
        return safe
      }
      const captureUsage = (value: Record<string, unknown>) => {
        const usage = record(value.usage)
        job.result.usage = Object.fromEntries(['input_tokens', 'output_tokens', 'total_tokens']
          .filter(key => typeof usage[key] === 'number' && Number.isFinite(usage[key]))
          .map(key => [key, usage[key] as number]))
      }
      const responseText = (value: Record<string, unknown>) => Array.isArray(value.output)
        ? value.output.flatMap((item) => {
            const content = record(item).content
            return Array.isArray(content) ? content.map(part => record(part).text).filter(text => typeof text === 'string') : []
          }).join('')
        : ''
      const event = (data: string) => {
        if (!data.trim() || data.trim() === '[DONE]')
          return
        const value = record(JSON.parse(data))
        if (value.type === 'response.output_text.delta' && typeof value.delta === 'string') {
          streamingText += value.delta
          job.result.text = streamText(streamingText)
          if (streamingText.length > 64000)
            throw new Error('回复超过测试页面的长度上限')
        }
        if (value.type === 'response.completed') {
          completed = true
          const output = record(value.response)
          job.result.text = clean(streamingText || responseText(output)).slice(0, 64000)
          captureUsage(output)
        }
        if (['response.failed', 'response.incomplete', 'error'].includes(String(value.type))) {
          const error = record(record(value.response).error ?? value.error)
          throw new Error(typeof error.message === 'string' ? clean(error.message).slice(0, 800) : '响应失败或未完成，请检查 RS 请求日志')
        }
      }
      try {
        for (;;) {
          const next = await reader.read()
          if (next.done)
            break
          bytes += next.value.byteLength
          if (bytes > 2 * 1024 * 1024)
            throw new Error('响应超过测试页面的 2 MB 上限')
          buffer += decoder.decode(next.value, { stream: true })
          if (sse) {
            let end = buffer.indexOf('\n')
            while (end >= 0) {
              const line = buffer.slice(0, end).replace(/\r$/, '')
              buffer = buffer.slice(end + 1)
              if (line.startsWith('data:'))
                event(line.slice(5))
              end = buffer.indexOf('\n')
            }
          }
        }
        buffer += decoder.decode()
      }
      finally {
        await reader.cancel().catch(() => {})
      }
      if (!response.ok) {
        let message = `HTTP ${response.status}：请检查 RS 请求日志`
        try {
          const error = record(record(JSON.parse(buffer)).error)
          if (typeof error.message === 'string')
            message = `HTTP ${response.status}：${clean(error.message).slice(0, 800)}`
        }
        catch {}
        throw new Error(message)
      }
      if (sse) {
        if (buffer.startsWith('data:'))
          event(buffer.slice(5))
        if (!completed)
          throw new Error('响应流未正常完成，请检查 RS 请求日志')
      }
      else {
        const value = record(JSON.parse(buffer))
        if (models) {
          if (!Array.isArray(value.data))
            throw new Error('模型列表格式异常')
          job.result.models = value.data.map(item => record(item).id)
            .filter((id): id is string => typeof id === 'string' && id.length <= 200)
            .slice(0, 1000)
            .map(clean)
        }
        else {
          job.result.text = clean(responseText(value).slice(0, 64000))
          captureUsage(value)
        }
      }
      if (job.result.state === 'running')
        job.result.state = 'completed'
    }
    catch (error) {
      if (job.result.state === 'running') {
        job.result.state = 'failed'
        job.result.error = clean(job.controller.signal.aborted ? '请求超过 120 秒或已取消' : error instanceof Error ? error.message : '请求失败')
      }
    }
    finally {
      secret = ''
      window.clearTimeout(timeout)
      job.result.elapsedMs = Math.round(performance.now() - start)
    }
  }

  async function invoke(method: string, value: unknown): Promise<unknown> {
    if (disposed)
      throw new Error('插件页面已关闭')
    const input = record(value)
    if (method === 'chat.context') {
      await permission()
      return { baseUrl: `${window.location.origin}${API_BASE_URL}/v1`, keys: await keys() }
    }
    if (method === 'chat.start') {
      if (typeof input.keyId !== 'string' || input.keyId.length > 128 || !['models', 'chat'].includes(String(input.kind)))
        throw new Error('请选择有效的 API Key')
      if (input.kind === 'chat') {
        if (typeof input.model !== 'string' || !input.model.trim() || input.model.length > 200
          || !Array.isArray(input.messages) || !input.messages.length || input.messages.length > 40
          || input.messages.some(item => !['user', 'assistant'].includes(String(record(item).role)) || typeof record(item).content !== 'string' || !(record(item).content as string).length || (record(item).content as string).length > 16000)
          || JSON.stringify(input.messages).length > 64000) {
          throw new Error('消息或模型格式无效，请开始新对话后重试')
        }
      }
      if ([...jobs.values()].some(job => job.result.state === 'running'))
        throw new Error('已有请求正在进行，请先停止')
      if (jobs.size >= 8)
        jobs.delete(jobs.keys().next().value!)
      const job = { result: { state: 'running', text: '' } as JobResult, controller: new AbortController() }
      const id = String(++sequence)
      jobs.set(id, job)
      void run(job, input)
      return { jobId: id }
    }
    const job = typeof input.jobId === 'string' ? jobs.get(input.jobId) : undefined
    if (!job)
      throw new Error('请求不存在或已过期')
    if (method === 'chat.cancel') {
      if (job.result.state === 'running') {
        job.result.state = 'cancelled'
        job.controller.abort()
      }
    }
    else if (method !== 'chat.poll') {
      throw new Error('不支持的页面操作')
    }
    if (job.renderedText !== job.result.text) {
      job.renderedHtml = renderChatMarkdown(job.result.text)
      job.renderedText = job.result.text
    }
    return { ...job.result, html: job.renderedHtml || '' }
  }
  return { invoke, dispose }
}

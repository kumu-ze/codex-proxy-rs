import type { RequestOptions } from '../request'
import request from '../request'

export type TicketMode = 'off' | 'manual' | 'auto'
export interface TicketPolicy { mode: TicketMode, targetLength: number | null }
export interface TicketSettings {
  enabled: boolean
  inject: boolean
  plusProLength: number
  businessLength: number
  defaultLength: number
  models: string[]
  intervalSeconds: number
  manualIntervalSeconds: number
  ttlSeconds: number
  refreshBeforeSeconds: number
  accounts: Record<string, TicketPolicy>
}
export interface TicketResult {
  accountId: string
  model: string
  httpStatus: number
  length: number
  matched: boolean
  checkedAt: number
  message: string
}
export interface TicketAccount {
  id: string
  name: string
  plan: string | null
  eligible: boolean
  targetLength: number
  policy: TicketPolicy
  models: { model: string, ready: boolean, expiresAt: number | null, lastResult: TicketResult | null, busy: boolean, retryAt: number | null, manualRetryAt: number | null }[]
}
export interface TicketLog {
  id: string
  accountName: string
  trigger: TicketMode
  proxyEndpoint: string
  proxyName: string
  targetLength: number
  startedAt: number
  durationMs: number
  retryAt: number | null
  result: TicketResult
}
export interface TicketProxyInput { id?: string, name: string, url?: string, savedProxyId?: string }
export interface TicketProxyView { id: string, name: string, endpoint: string, hasAuthentication: boolean }
export interface TicketPanel { revision: number, settings: TicketSettings, proxyConfigured: boolean, proxyCount: number, proxies: TicketProxyView[], accounts: TicketAccount[], logs: TicketLog[], logLimit: number }

export function getTicketPanel(options: RequestOptions = {}) {
  return request<TicketPanel>({
    url: '/api/admin/tickets',
    method: 'GET',
    ...options,
  })
}
export function saveTicketSettings(data: { revision: number, settings: TicketSettings, proxyUrl?: string, proxyPool?: string[], proxies?: TicketProxyInput[] }) {
  return request<TicketPanel>({
    url: '/api/admin/tickets',
    method: 'POST',
    data,
  })
}
export function probeTicket(data: { accountId: string, model: string }) {
  return request<TicketResult>({
    url: '/api/admin/tickets/probe',
    method: 'POST',
    data,
    timeout: 35000,
  })
}

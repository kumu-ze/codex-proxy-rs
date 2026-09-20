import request from '../request'

export interface PluginStatus {
  id: string
  version: string
  available: boolean
  enabled: boolean
  menuLabel: string | null
}

export function getPlugins() {
  return request<PluginStatus[]>({
    url: '/api/admin/plugins',
    method: 'GET',
    silent: true,
  })
}

export function invokePlugin(id: string, method: string, input: unknown) {
  return request<unknown>({
    url: '/api/admin/plugins/invoke',
    method: 'POST',
    data: { id, method, input },
  })
}

export type PluginManagement = { action: 'install', url: string, sha256?: string, proxyId?: string } | { action: 'enable' | 'disable' | 'uninstall', id: string }

export function managePlugin(operation: PluginManagement) {
  return request<PluginStatus[]>({
    url: '/api/admin/plugins/manage',
    method: 'POST',
    data: operation,
    timeout: 120000,
  })
}

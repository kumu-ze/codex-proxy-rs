import request from '../request'

export interface PluginStatus {
  id: string
  version: string
  available: boolean
}

export function getPlugins() {
  return request<PluginStatus[]>({
    url: '/api/admin/plugins',
    method: 'GET',
  })
}

export function invokePlugin(id: string, method: string, input: unknown) {
  return request<unknown>({
    url: '/api/admin/plugins/invoke',
    method: 'POST',
    data: { id, method, input },
  })
}

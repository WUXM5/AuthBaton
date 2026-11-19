import type { EngineInterface, Register } from 'claude-code'

// AuthBaton local Unix socket for Claude Desktop Code tab seamless same-Team relay.
// No credential ever enters this mod: it strips Authorization and hands only engine /v1/messages
// traffic to AuthBaton over a private 0600 Unix socket, falling back to official direct requests
// whenever AuthBaton is stopped, disabled, or refuses cross-Team traffic.
const SOCKET_SUFFIX = '/Library/Application Support/AuthManager/claude-desktop-relay.sock'
const HEALTH_TTL_MS = 2_000
const STRIP_HEADERS = new Set([
  'authorization',
  'x-api-key',
  'host',
  'content-length',
  'transfer-encoding',
  'connection',
])

type HealthReply = {
  active?: boolean
  label?: string
}

type RelayState = {
  isDesktop: boolean | null
  socketPath: string | null
  healthAt: number
  active: boolean
  label: string
  statusShown: boolean
}

function getHeader(
  headers: Readonly<Record<string, string>> | undefined,
  name: string,
): string | undefined {
  if (!headers) {
    return undefined
  }
  const target = name.toLowerCase()
  for (const [k, v] of Object.entries(headers)) {
    if (k.toLowerCase() === target) {
      return v
    }
  }
  return undefined
}

function decodeHeaderLabel(raw: string | undefined): string {
  if (!raw) {
    return ''
  }
  try {
    return decodeURIComponent(raw)
  } catch {
    return raw
  }
}

function cleanForwardHeaders(
  headers: Readonly<Record<string, string>> | undefined,
): Record<string, string> {
  const out: Record<string, string> = {}
  if (!headers) {
    return out
  }
  for (const [k, v] of Object.entries(headers)) {
    if (!STRIP_HEADERS.has(k.toLowerCase())) {
      out[k] = v
    }
  }
  return out
}

function parseMessagesTarget(rawUrl: string): string | undefined {
  try {
    const parsed = new URL(rawUrl)
    // Intercept only /v1/messages (with optional query like ?beta=true); skip /v1/messages/count_tokens.
    if (parsed.pathname !== '/v1/messages') {
      return undefined
    }
    return `http://authbaton${parsed.pathname}${parsed.search}`
  } catch {
    return undefined
  }
}

async function resolveDesktop(state: RelayState, $: EngineInterface): Promise<boolean> {
  if (state.isDesktop === null) {
    const entry = await $.env.get('CLAUDE_CODE_ENTRYPOINT')
    state.isDesktop = entry === 'claude-desktop'
  }
  return state.isDesktop
}

async function resolveSocket(state: RelayState, $: EngineInterface): Promise<string | undefined> {
  if (state.socketPath === null) {
    const home = await $.env.get('HOME')
    state.socketPath = home ? `${home}${SOCKET_SUFFIX}` : ''
  }
  return state.socketPath || undefined
}

function setStatus(state: RelayState, $: EngineInterface, label?: string) {
  if (label) {
    $.ui.status(`当前账号：${label}`)
    state.statusShown = true
  } else if (state.statusShown) {
    $.ui.status(undefined)
    state.statusShown = false
  }
}

async function checkHealth(state: RelayState, $: EngineInterface, socketPath: string): Promise<boolean> {
  const now = await $.clock.now()
  if (now - state.healthAt < HEALTH_TTL_MS) {
    return state.active
  }
  state.healthAt = now
  try {
    const res = await $.http.fetch('http://authbaton/v1/health', {
      method: 'GET',
      socketPath,
    })
    if (res.ok) {
      const data = JSON.parse(res.text) as HealthReply
      if (data.active === true) {
        state.active = true
        state.label = typeof data.label === 'string' ? data.label : ''
        setStatus(state, $, state.label || '已接力')
        return true
      }
    }
  } catch {
    // AuthBaton is not running or unreachable: Fail-Open.
  }
  state.active = false
  state.label = ''
  setStatus(state, $, undefined)
  return false
}

export const register: Register = on => {
  const state: RelayState = {
    isDesktop: null,
    socketPath: null,
    healthAt: -Infinity,
    active: false,
    label: '',
    statusShown: false,
  }

  on('http.fetch', async ($, e, next) => {
    if (next.origin.plugin !== 'engine') {
      return next(e)
    }
    if ((e.init?.method ?? 'GET').toUpperCase() !== 'POST') {
      return next(e)
    }
    const targetUrl = parseMessagesTarget(e.url)
    if (!targetUrl) {
      return next(e)
    }
    if (!(await resolveDesktop(state, $))) {
      return next(e)
    }
    const socketPath = await resolveSocket(state, $)
    if (!socketPath) {
      return next(e)
    }
    if (!(await checkHealth(state, $, socketPath))) {
      return next(e)
    }

    try {
      const res = await next({
        url: targetUrl,
        init: {
          ...e.init,
          headers: cleanForwardHeaders(e.init?.headers),
          socketPath,
        },
      })
      const value = 'value' in res ? res.value : undefined
      if (value) {
        if (value.status === 503 && getHeader(value.headers, 'x-authbaton-pass') === '1') {
          state.healthAt = -Infinity
          state.active = false
          state.label = ''
          setStatus(state, $, undefined)
          return next(e)
        }
        const accountLabel = decodeHeaderLabel(getHeader(value.headers, 'x-authbaton-account'))
        if (accountLabel) {
          state.label = accountLabel
          setStatus(state, $, accountLabel)
        }
        const switchedFrom = decodeHeaderLabel(getHeader(value.headers, 'x-authbaton-switched-from'))
        if (switchedFrom && accountLabel) {
          $.ui.toast(`原账号额度已满，已无感切到 ${accountLabel}，对话继续`)
        }
      }
      return res
    } catch {
      // Relay became unreachable mid-flight before a response arrived: Fail-Open.
      state.healthAt = -Infinity
      state.active = false
      state.label = ''
      setStatus(state, $, undefined)
      return next(e)
    }
  })
}

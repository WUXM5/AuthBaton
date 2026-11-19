import type { EngineInterface, Register } from 'claude-code'

// AuthBaton local Unix socket for Claude Desktop Code tab seamless same-Team account switching.
// Requests stay on Claude Desktop's own built-in Bun engine (native TLS/HTTP/proxy stack):
// this mod only leases a short-lived same-Team access token over AuthBaton's 0600 local socket,
// swaps Authorization + metadata.user_id.account_uuid in place, handles 429 failover via a second
// next() call, and Fails-Open to the unmodified Desktop request whenever AuthBaton is inactive.
const SOCKET_SUFFIX = '/Library/Application Support/AuthManager/claude-desktop-relay.sock'
const MAX_ATTEMPTS = 3
const REDACTED = '[redacted]'

type LeaseReply = {
  active?: boolean
  id?: string
  label?: string
  account_uuid?: string
  access_token?: string
  has_fallback?: boolean
}

type SwapState = {
  isDesktop: boolean | null
  socketPath: string | null
  stripRedacted: boolean | null
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

function parseFloatHeader(
  headers: Readonly<Record<string, string>> | undefined,
  name: string,
): number | undefined {
  const raw = getHeader(headers, name)
  if (raw === undefined) {
    return undefined
  }
  const parsed = Number.parseFloat(raw)
  return Number.isFinite(parsed) ? parsed : undefined
}

function isMessagesUrl(rawUrl: string): boolean {
  try {
    // Intercept only /v1/messages (including ?beta=true); skip /v1/messages/count_tokens.
    return new URL(rawUrl).pathname === '/v1/messages'
  } catch {
    return false
  }
}

function skipWs(text: string, i: number): number {
  while (i < text.length) {
    const c = text.charCodeAt(i)
    if (c === 32 || c === 9 || c === 13 || c === 10) {
      i += 1
    } else {
      break
    }
  }
  return i
}

function scanJsonString(text: string, start: number): [number, number] | undefined {
  if (start >= text.length || text.charCodeAt(start) !== 34) {
    return undefined
  }
  let i = start + 1
  while (i < text.length) {
    const c = text.charCodeAt(i)
    if (c === 92) {
      i += 2
    } else if (c === 34) {
      return [start, i + 1]
    } else {
      i += 1
    }
  }
  return undefined
}

function skipJsonValue(text: string, start: number): number | undefined {
  const i = skipWs(text, start)
  if (i >= text.length) {
    return undefined
  }
  const first = text.charCodeAt(i)
  if (first === 34) {
    const span = scanJsonString(text, i)
    return span ? span[1] : undefined
  }
  if (first === 123 || first === 91) {
    let pos = i
    let depth = 0
    while (pos < text.length) {
      const c = text.charCodeAt(pos)
      if (c === 34) {
        const span = scanJsonString(text, pos)
        if (!span) {
          return undefined
        }
        pos = span[1]
      } else if (c === 123 || c === 91) {
        depth += 1
        pos += 1
      } else if (c === 125 || c === 93) {
        depth -= 1
        pos += 1
        if (depth === 0) {
          return pos
        }
      } else {
        pos += 1
      }
    }
    return undefined
  }
  let pos = i
  while (pos < text.length) {
    const c = text.charCodeAt(pos)
    if (
      c === 44 ||
      c === 125 ||
      c === 93 ||
      c === 32 ||
      c === 9 ||
      c === 13 ||
      c === 10
    ) {
      break
    }
    pos += 1
  }
  return pos > i ? pos : undefined
}

function findStringFieldInObject(
  text: string,
  start: number,
  targetKey: string,
): [number, number] | undefined {
  let i = skipWs(text, start)
  if (i >= text.length || text.charCodeAt(i) !== 123) {
    return undefined
  }
  i = skipWs(text, i + 1)
  while (i < text.length) {
    if (text.charCodeAt(i) === 125) {
      return undefined
    }
    const keySpan = scanJsonString(text, i)
    if (!keySpan) {
      return undefined
    }
    let key: string
    try {
      key = JSON.parse(text.slice(keySpan[0], keySpan[1])) as string
    } catch {
      return undefined
    }
    i = skipWs(text, keySpan[1])
    if (i >= text.length || text.charCodeAt(i) !== 58) {
      return undefined
    }
    i = skipWs(text, i + 1)
    if (key === targetKey) {
      return scanJsonString(text, i)
    }
    const nextPos = skipJsonValue(text, i)
    if (nextPos === undefined) {
      return undefined
    }
    i = skipWs(text, nextPos)
    if (i < text.length && text.charCodeAt(i) === 44) {
      i = skipWs(text, i + 1)
    } else {
      return undefined
    }
  }
  return undefined
}

// Finds exact [start, end) span of top-level root["metadata"]["user_id"] JSON string literal.
function findMetadataUserIdSpan(text: string): [number, number] | undefined {
  let i = skipWs(text, 0)
  if (i >= text.length || text.charCodeAt(i) !== 123) {
    return undefined
  }
  i = skipWs(text, i + 1)
  while (i < text.length) {
    if (text.charCodeAt(i) === 125) {
      return undefined
    }
    const keySpan = scanJsonString(text, i)
    if (!keySpan) {
      return undefined
    }
    let key: string
    try {
      key = JSON.parse(text.slice(keySpan[0], keySpan[1])) as string
    } catch {
      return undefined
    }
    i = skipWs(text, keySpan[1])
    if (i >= text.length || text.charCodeAt(i) !== 58) {
      return undefined
    }
    i = skipWs(text, i + 1)
    if (key === 'metadata') {
      return findStringFieldInObject(text, i, 'user_id')
    }
    const nextPos = skipJsonValue(text, i)
    if (nextPos === undefined) {
      return undefined
    }
    i = skipWs(text, nextPos)
    if (i < text.length && text.charCodeAt(i) === 44) {
      i = skipWs(text, i + 1)
    } else {
      return undefined
    }
  }
  return undefined
}

function extractModel(body: string | { readonly base64: string } | undefined): string | undefined {
  if (typeof body !== 'string') {
    return undefined
  }
  const span = findStringFieldInObject(body, 0, 'model')
  if (!span) {
    return undefined
  }
  try {
    const parsed = JSON.parse(body.slice(span[0], span[1])) as unknown
    return typeof parsed === 'string' && parsed.length <= 128 ? parsed : undefined
  } catch {
    return undefined
  }
}

// Rewrites only top-level metadata.user_id.account_uuid in place, keeping thinking blocks,
// signatures, system/tools, and cache_control byte-for-byte intact.
function rewriteMetadataUserId(
  body: string | { readonly base64: string } | undefined,
  targetAccountUuid: string,
): { body: string | { readonly base64: string } | undefined; rewritten: boolean } {
  if (typeof body !== 'string' || !targetAccountUuid) {
    return { body, rewritten: false }
  }
  const span = findMetadataUserIdSpan(body)
  if (!span) {
    return { body, rewritten: false }
  }
  let userId: string
  try {
    const parsed = JSON.parse(body.slice(span[0], span[1])) as unknown
    if (typeof parsed !== 'string') {
      return { body, rewritten: false }
    }
    userId = parsed
  } catch {
    return { body, rewritten: false }
  }

  let updated: string | undefined
  if (userId.trimStart().startsWith('{')) {
    try {
      const uidObj = JSON.parse(userId) as Record<string, unknown>
      if (typeof uidObj.account_uuid === 'string' && uidObj.account_uuid !== targetAccountUuid) {
        uidObj.account_uuid = targetAccountUuid
        updated = JSON.stringify(uidObj)
      }
    } catch {
      return { body, rewritten: false }
    }
  } else {
    const acctIdx = userId.indexOf('_account_')
    if (acctIdx >= 0) {
      const afterAcct = acctIdx + '_account_'.length
      const sessIdx = userId.indexOf('_session_', afterAcct)
      if (sessIdx >= 0) {
        const current = userId.slice(afterAcct, sessIdx)
        if (current !== targetAccountUuid) {
          updated = `${userId.slice(0, afterAcct)}${targetAccountUuid}${userId.slice(sessIdx)}`
        }
      }
    }
  }

  if (updated === undefined) {
    return { body, rewritten: false }
  }
  const newLiteral = JSON.stringify(updated)
  return {
    body: body.slice(0, span[0]) + newLiteral + body.slice(span[1]),
    rewritten: true,
  }
}

function buildSwappedHeaders(
  headers: Readonly<Record<string, string>> | undefined,
  accessToken: string,
  stripRedacted: boolean,
): Record<string, string> {
  const out: Record<string, string> = {}
  if (headers) {
    for (const [k, v] of Object.entries(headers)) {
      const lower = k.toLowerCase()
      if (lower === 'authorization' || lower === 'x-api-key') {
        continue
      }
      if (stripRedacted && v === REDACTED) {
        continue
      }
      out[k] = v
    }
  }
  out['authorization'] = `Bearer ${accessToken}`
  return out
}

async function resolveDesktop(state: SwapState, $: EngineInterface): Promise<boolean> {
  if (state.isDesktop === null) {
    const entry = await $.env.get('CLAUDE_CODE_ENTRYPOINT')
    state.isDesktop = entry === 'claude-desktop'
  }
  return state.isDesktop
}

async function resolveSocket(state: SwapState, $: EngineInterface): Promise<string | undefined> {
  if (state.socketPath === null) {
    const home = await $.env.get('HOME')
    state.socketPath = home ? `${home}${SOCKET_SUFFIX}` : ''
  }
  return state.socketPath || undefined
}

async function resolveStripRedacted(state: SwapState, $: EngineInterface): Promise<boolean> {
  if (state.stripRedacted === null) {
    const flag = await $.env.get('CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC')
    state.stripRedacted = flag === '1' || flag === 'true'
  }
  return state.stripRedacted
}

function setStatus(state: SwapState, $: EngineInterface, label?: string) {
  if (label) {
    $.ui.status(`当前账号：${label}`)
    state.statusShown = true
  } else if (state.statusShown) {
    $.ui.status(undefined)
    state.statusShown = false
  }
}

async function fetchLease(
  $: EngineInterface,
  socketPath: string,
  exclude: string[],
  model?: string,
): Promise<LeaseReply | undefined> {
  try {
    const res = await $.http.fetch('http://authbaton/v1/lease', {
      method: 'POST',
      socketPath,
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ exclude, model }),
    })
    if (!res.ok) {
      return undefined
    }
    return JSON.parse(res.text) as LeaseReply
  } catch {
    return undefined
  }
}

async function reportOutcome(
  $: EngineInterface,
  socketPath: string,
  payload: Record<string, unknown>,
): Promise<void> {
  try {
    await $.http.fetch('http://authbaton/v1/outcome', {
      method: 'POST',
      socketPath,
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(payload),
    })
  } catch {
    // Non-fatal: AuthBaton may have stopped while the turn was running.
  }
}

export const register: Register = on => {
  const state: SwapState = {
    isDesktop: null,
    socketPath: null,
    stripRedacted: null,
    statusShown: false,
  }

  on('http.fetch', async ($, e, next) => {
    if (next.origin.plugin !== 'engine') {
      return next(e)
    }
    if ((e.init?.method ?? 'GET').toUpperCase() !== 'POST') {
      return next(e)
    }
    if (!isMessagesUrl(e.url)) {
      return next(e)
    }
    if (!(await resolveDesktop(state, $))) {
      return next(e)
    }
    const socketPath = await resolveSocket(state, $)
    if (!socketPath) {
      return next(e)
    }

    const model = extractModel(e.init?.body)
    const tried: string[] = []
    const firstLease = await fetchLease($, socketPath, tried, model)
    if (!firstLease?.active || !firstLease.id || !firstLease.access_token || !firstLease.account_uuid) {
      setStatus(state, $, undefined)
      return next(e)
    }

    const stripRedacted = await resolveStripRedacted(state, $)
    let lease: LeaseReply = firstLease
    const firstLabel = firstLease.label || '当前账号'

    for (let attempt = 0; attempt < MAX_ATTEMPTS; attempt += 1) {
      const accountId = lease.id!
      const label = lease.label || '当前账号'
      tried.push(accountId)
      setStatus(state, $, label)

      const { body: nextBody, rewritten } = rewriteMetadataUserId(e.init?.body, lease.account_uuid!)
      const headers = buildSwappedHeaders(e.init?.headers, lease.access_token!, stripRedacted)
      const started = await $.clock.now()
      const res = await next({
        url: e.url,
        init: {
          ...e.init,
          headers,
          body: nextBody,
        },
      })
      const elapsedMs = Math.max(0, (await $.clock.now()) - started)
      const value = 'value' in res ? res.value : undefined
      const status = value?.status ?? 0
      const rlStatus = getHeader(value?.headers, 'anthropic-ratelimit-unified-status')
      const rl5h = parseFloatHeader(value?.headers, 'anthropic-ratelimit-unified-5h-utilization')
      const rl7d = parseFloatHeader(value?.headers, 'anthropic-ratelimit-unified-7d-utilization')

      // Transparent in-request failover on 429 using Claude Code's native multi-send support.
      if (status === 429 && lease.has_fallback && attempt + 1 < MAX_ATTEMPTS) {
        await reportOutcome($, socketPath, {
          id: accountId,
          status: 429,
          model,
          ms: elapsedMs,
          uid_rewritten: rewritten,
          switched: attempt > 0,
          rl_status: rlStatus,
          rl_5h: rl5h,
          rl_7d: rl7d,
        })
        const fallback = await fetchLease($, socketPath, tried, model)
        if (fallback?.active && fallback.id && fallback.access_token && fallback.account_uuid) {
          lease = fallback
          continue
        }
      }

      const switched = attempt > 0
      if (switched) {
        setStatus(state, $, label)
        $.ui.toast(`原账号额度已满，已无感切到 ${label}，对话继续`)
      }
      void reportOutcome($, socketPath, {
        id: accountId,
        status,
        model,
        ms: elapsedMs,
        uid_rewritten: rewritten,
        switched,
        switched_from: switched ? firstLabel : undefined,
        rl_status: rlStatus,
        rl_5h: rl5h,
        rl_7d: rl7d,
      })
      return res
    }

    return next(e)
  })
}

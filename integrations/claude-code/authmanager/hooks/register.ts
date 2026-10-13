import type { EngineInterface, Register, SessionRateLimit, Timer } from 'claude-code'

// AuthManager's endpoint under $HOME; the host only speaks HTTP over a Unix socket.
const SOCKET = '/Library/Application Support/AuthManager/claude-mod.sock'
// Only the terminal CLI and `claude -p` follow the default login. The desktop app injects its
// own token, so its sessions must not report its quota or be resumed here. A `-p` run exits
// as its turn ends, before the login cache below expires, so it only reports.
const ENTRYPOINTS = ['cli', 'sdk-cli']
const RESUMES = 'cli'
const PRIMARY = ['five_hour', 'seven_day']
// A running CLI caches the Keychain login for 30 s; resuming earlier hits the old account again.
const AFTER_SWITCH_MS = 35_000
const AFTER_RESET_MS = 5_000
// A change this long before the limit was already in effect, so it cannot be the cure for it.
const MATCH_MS = 40_000
const POLL_MS = 5_000
const GIVE_UP_MS = 600_000
const CONTINUE =
  'Usage is available again. Continue the task you were working on when the limit was reached; do not repeat work that is already complete.'

type Change = { at: number; kind: 'switch' | 'reset'; label: string }
type Reply = { account: string | null; change: Change | null }
type Episode = { limitedAt: number; deadline: number; poll?: Timer; resume?: Timer }
type State = {
  entry: string | undefined | null
  windows: SessionRateLimit[]
  episode?: Episode
  // The limit AuthManager's switch or reset resumed. Claude Code keeps one timed resume per session and
  // moves it to each new limit; a plugin prompt does not cancel it. So it is this limit's to drop, once,
  // until a later limit takes it over.
  resumed?: Episode
}

async function active(s: State, $: EngineInterface): Promise<boolean> {
  if (s.entry === null) {
    s.entry = await $.env.get('CLAUDE_CODE_ENTRYPOINT')
  }
  return s.entry !== undefined && ENTRYPOINTS.includes(s.entry)
}

async function report(s: State, $: EngineInterface, limited: boolean): Promise<Reply | undefined> {
  try {
    const home = await $.env.get('HOME')
    const response = await $.http.fetch('http://authmanager/v1/report', {
      method: 'POST',
      socketPath: `${home}${SOCKET}`,
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({
        limited,
        windows: s.windows
          .filter(w => PRIMARY.includes(w.kind))
          .map(w => ({ kind: w.kind, used: w.percentUsed })),
      }),
    })
    return response.ok ? (JSON.parse(response.text) as Reply) : undefined
  } catch {
    return undefined
  }
}

function end(s: State, $: EngineInterface, notice?: string) {
  s.episode?.poll?.cancel()
  s.episode?.resume?.cancel()
  s.episode = undefined
  $.ui.status(undefined)
  if (notice !== undefined) {
    $.ui.toast(notice)
  }
}

async function resume(s: State, $: EngineInterface) {
  s.resumed = s.episode
  end(s, $)
  await $.prompt.submit({ text: CONTINUE, asUser: true })
}

async function check(s: State, $: EngineInterface) {
  const reply = await report(s, $, true)
  const episode = s.episode
  if (episode === undefined || episode.resume !== undefined) {
    return
  }
  if (reply === undefined) {
    return end(s, $) // AuthManager is not running: Claude Code's own resume stays in charge.
  }
  const now = await $.clock.now()
  const change = reply.change
  if (change !== null && change.at * 1000 >= episode.limitedAt - MATCH_MS) {
    episode.poll?.cancel()
    const wait = Math.max(
      0,
      change.at * 1000 + (change.kind === 'switch' ? AFTER_SWITCH_MS : AFTER_RESET_MS) - now,
    )
    $.ui.status(
      change.kind === 'switch'
        ? `已换到 ${change.label}，约 ${Math.ceil(wait / 1000)} 秒后自动继续`
        : `${change.label} 已重置额度，马上继续`,
    )
    episode.resume = $.clock.after(wait, () => void resume(s, $))
  } else if (now >= episode.deadline) {
    end(s, $, 'AuthManager 没有换号或重置；额度恢复时 Claude Code 会自己继续')
  }
}

async function startEpisode(s: State, $: EngineInterface) {
  const now = await $.clock.now()
  s.episode = { limitedAt: now, deadline: now + GIVE_UP_MS }
  s.resumed = undefined
  $.ui.status('额度见底，等待 AuthManager 换号或重置…')
  await check(s, $)
  if (s.episode !== undefined && s.episode.resume === undefined) {
    s.episode.poll = $.clock.every(POLL_MS, () => void check(s, $))
  }
}

export const register: Register = on => {
  const s: State = { entry: null, windows: [] }

  on('session.measure', async ($, e, next) => {
    const result = await next(e)
    if (e.changed.includes('rateLimits') && (await active(s, $))) {
      s.windows = [...e.rateLimits]
      void report(s, $, false)
    }
    return result
  })

  on('turn.complete', async ($, e, next) => {
    const result = await next(e)
    if (e.reason !== 'error' || e.agentId !== undefined || s.episode !== undefined || !(await active(s, $))) {
      return result
    }
    // The interactive CLI ends the turn before the measure for its last response arrives.
    s.windows = [...(await $.session.usage()).rateLimits]
    if (s.windows.some(w => PRIMARY.includes(w.kind) && w.percentUsed >= 100)) {
      if (s.entry === RESUMES) {
        await startEpisode(s, $)
      } else {
        await report(s, $, true)
      }
    }
    return result
  })

  on('prompt.submit', ($, e, next) => {
    // An origin left out means the person's own prompt.
    const origin = e.origin?.kind ?? 'composer'
    if (origin === 'auto-continuation' && s.resumed !== undefined && /usage/i.test(e.text)) {
      s.resumed = undefined
      return { drop: 'AuthManager 已换号并续上了这个任务，跳过额度重置时的自动继续' }
    }
    if (['composer', 'sdk', 'bridge'].includes(origin)) {
      // The person's own prompt takes over, and Claude Code cancels its timed resume too.
      s.resumed = undefined
      if (s.episode !== undefined) {
        end(s, $)
      }
    }
    return next(e)
  })
}

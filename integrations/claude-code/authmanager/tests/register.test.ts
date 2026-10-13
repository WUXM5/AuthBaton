import type {
  HttpInit,
  On,
  SessionMeasureInput,
  SessionRateLimit,
  SessionUsage,
  TurnCompleteInput,
} from 'claude-code'
import { describe, expect, mock, test } from 'claude-code/testing'

const T0 = 1_800_000_000_000

type Change = { at: number; kind: 'switch' | 'reset'; label: string } | null

// Everything beneath the plugin: the environment, the clock, and AuthManager's socket.
function world(on: On, entry: string, change: () => Change | Error) {
  const seen = {
    posts: [] as { socket?: string; body: { limited: boolean; windows: unknown[] } }[],
    status: [] as (string | undefined)[],
    toasts: [] as string[],
    prompts: [] as { text: string; asUser?: true }[],
  }
  // What the last response reported, as `$.session.usage()` reads it.
  let limits: SessionRateLimit[] = []
  mock.env(on, { CLAUDE_CODE_ENTRYPOINT: entry, HOME: '/Users/someone' })
  const clock = mock.clock(on, { now: T0 })
  on('http.fetch', ($, e) => {
    const init = e.init as HttpInit
    seen.posts.push({ socket: init.socketPath, body: JSON.parse(init.body as string) })
    const answer = change()
    if (answer instanceof Error) {
      throw answer
    }
    const text = JSON.stringify({ account: 'A', change: answer })
    return { value: { status: 200, ok: true, headers: {}, text } }
  })
  on('ui.status', ($, e) => {
    seen.status.push(e.text)
    return { value: undefined }
  })
  on('ui.toast', ($, e) => {
    seen.toasts.push(e.text)
    return { value: undefined }
  })
  on('prompt.submit', ($, e) => {
    seen.prompts.push({ text: e.text })
    return { text: e.text }
  })
  on('session.measure', ($, e) => {
    limits = [...e.rateLimits]
    return { changed: e.changed }
  })
  on('session.usage', () => ({
    value: { startedAt: T0, context: {} as SessionUsage['context'], rateLimits: limits },
  }))
  on('turn.complete', () => ({ text: '' }))
  return { seen, clock, reported: (next: SessionRateLimit[]) => (limits = next) }
}

const measure = (used: number): SessionMeasureInput => ({
  context: {} as SessionMeasureInput['context'],
  rateLimits: [
    { kind: 'five_hour', percentUsed: used },
    { kind: 'seven_day', percentUsed: 40 },
    { kind: 'spend_limit', percentUsed: 120 },
  ],
  changed: ['rateLimits'],
})
const failed: TurnCompleteInput = {
  reason: 'error',
  answer: '',
  durationMs: 1,
  isAborted: false,
  turnId: 't1',
}
// Claude Code's own resume at the reset time. It keeps one per session and moves it to each new limit.
const official = {
  text: 'Your claude.ai usage limit has reset. Continue the task you were working on when the limit was reached; do not repeat work that is already complete.',
  wait: false,
  origin: { kind: 'auto-continuation' as const },
}
const DROPPED = { drop: 'AuthManager 已换号并续上了这个任务，跳过额度重置时的自动继续' }

describe('authmanager', () => {
  test('desktop sessions neither report nor resume', async ($, on) => {
    const { seen } = world(on, 'claude-desktop', () => null)
    await $.session.measure(measure(100))
    await $.turn.complete(failed)
    expect(seen.posts).toEqual([])
    expect(seen.status).toEqual([])
  })

  test('readings are relayed with the primary windows only', async ($, on) => {
    const { seen, clock } = world(on, 'cli', () => null)
    await $.session.measure(measure(97))
    await clock.settle()
    expect(seen.posts).toEqual([
      {
        socket: '/Users/someone/Library/Application Support/AuthManager/claude-mod.sock',
        body: {
          limited: false,
          windows: [
            { kind: 'five_hour', used: 97 },
            { kind: 'seven_day', used: 40 },
          ],
        },
      },
    ])
  })

  test('a limited turn resumes 35 s after AuthManager switches', async ($, on) => {
    let change: Change = null
    const { seen, clock } = world(on, 'cli', () => change)
    await $.session.measure(measure(100))
    await $.turn.complete(failed)
    expect(seen.posts.at(-1)?.body.limited).toBe(true)
    expect(seen.status.at(-1)).toBe('额度见底，等待 AuthManager 换号或重置…')
    // The switch lands on the next poll; the CLI's 30 s login cache decides the wait.
    change = { at: (T0 + 3_000) / 1000, kind: 'switch', label: 'B' }
    await clock.advance(5_000)
    expect(seen.status.at(-1)).toBe('已换到 B，约 33 秒后自动继续')
    await clock.advance(32_000)
    expect(seen.prompts).toEqual([])
    await clock.advance(1_000)
    expect(seen.prompts).toHaveLength(1)
    expect(seen.prompts[0]?.text).toContain('do not repeat work that is already complete')
    expect(seen.status.at(-1)).toBe(undefined)
    // Polling stopped with the resume.
    const posts = seen.posts.length
    await clock.advance(60_000)
    expect(seen.posts).toHaveLength(posts)
  })

  test('the limit is read at the end of the turn, before its measure arrives', async ($, on) => {
    const { seen, clock, reported } = world(on, 'cli', () => ({ at: T0 / 1000, kind: 'switch', label: 'B' }))
    reported([{ kind: 'five_hour', percentUsed: 100 }])
    await $.turn.complete(failed)
    expect(seen.posts).toEqual([
      { socket: expect.any(String), body: { limited: true, windows: [{ kind: 'five_hour', used: 100 }] } },
    ])
    await clock.advance(35_000)
    expect(seen.prompts).toHaveLength(1)
  })

  test('a reset of the signed-in account resumes almost at once', async ($, on) => {
    const { seen, clock } = world(on, 'cli', () => ({ at: T0 / 1000, kind: 'reset', label: 'A' }))
    await $.session.measure(measure(100))
    await $.turn.complete(failed)
    await clock.advance(5_000)
    expect(seen.prompts).toHaveLength(1)
  })

  test('a change from before the limit is not taken as its cure', async ($, on) => {
    const old: Change = { at: (T0 - 60_000) / 1000, kind: 'switch', label: 'B' }
    const { seen, clock } = world(on, 'cli', () => old)
    await $.session.measure(measure(100))
    await $.turn.complete(failed)
    await clock.advance(600_000)
    expect(seen.prompts).toEqual([])
    expect(seen.toasts).toEqual(['AuthManager 没有换号或重置；额度恢复时 Claude Code 会自己继续'])
    expect(seen.status.at(-1)).toBe(undefined)
  })

  test('without AuthManager the plugin steps aside', async ($, on) => {
    const { seen, clock } = world(on, 'cli', () => new Error('ECONNREFUSED'))
    await $.session.measure(measure(100))
    await $.turn.complete(failed)
    await clock.advance(120_000)
    expect(seen.prompts).toEqual([])
    expect(seen.status.at(-1)).toBe(undefined)
  })

  test('the person typing takes over from the wait', async ($, on) => {
    const { seen, clock } = world(on, 'cli', () => null)
    await $.session.measure(measure(100))
    await $.turn.complete(failed)
    await $.prompt.submit({ text: 'go on with B', wait: false, origin: { kind: 'composer' } })
    const posts = seen.posts.length
    await clock.advance(60_000)
    expect(seen.posts).toHaveLength(posts)
    expect(seen.status.at(-1)).toBe(undefined)
    expect(seen.prompts).toEqual([{ text: 'go on with B' }])
  })

  test("after resuming, Claude Code's own resume at the reset time is dropped once", async ($, on) => {
    const { seen, clock } = world(on, 'cli', () => ({ at: T0 / 1000, kind: 'switch', label: 'B' }))
    await $.session.measure(measure(100))
    await $.turn.complete(failed)
    await clock.advance(40_000)
    expect(seen.prompts).toHaveLength(1)
    expect(await $.prompt.submit(official)).toEqual(DROPPED)
    await $.prompt.submit(official)
    expect(seen.prompts).toHaveLength(2)
  })

  test("a later limit AuthManager cannot cure keeps Claude Code's own resume for it", async ($, on) => {
    const { seen, clock } = world(on, 'cli', () => ({ at: T0 / 1000, kind: 'switch', label: 'B' }))
    await $.session.measure(measure(100))
    await $.turn.complete(failed)
    await clock.advance(40_000)
    expect(seen.prompts).toHaveLength(1)
    // B runs out too, before A's reset, and AuthManager has nothing left to change.
    await clock.advance(1_200_000)
    await $.turn.complete(failed)
    await clock.advance(600_000)
    expect(seen.toasts).toEqual(['AuthManager 没有换号或重置；额度恢复时 Claude Code 会自己继续'])
    // Claude Code's one resume now belongs to B's limit, which nothing resumed.
    await $.prompt.submit(official)
    expect(seen.prompts.map(p => p.text)).toEqual([expect.stringContaining('Usage is available again'), official.text])
  })

  test("two limits cured in one session drop Claude Code's single resume once", async ($, on) => {
    let change: Change = { at: T0 / 1000, kind: 'switch', label: 'B' }
    const { seen, clock } = world(on, 'cli', () => change)
    await $.session.measure(measure(100))
    await $.turn.complete(failed)
    await clock.advance(40_000)
    await clock.advance(1_200_000)
    change = { at: (T0 + 1_240_000) / 1000, kind: 'switch', label: 'C' }
    await $.turn.complete(failed)
    await clock.advance(40_000)
    expect(seen.prompts).toHaveLength(2)
    expect(await $.prompt.submit(official)).toEqual(DROPPED)
    await $.prompt.submit(official)
    expect(seen.prompts).toHaveLength(3)
  })

  test('a -p run reports its limit but cannot outlive the login cache to resume', async ($, on) => {
    const { seen, clock } = world(on, 'sdk-cli', () => ({ at: T0 / 1000, kind: 'switch', label: 'B' }))
    await $.session.measure(measure(100))
    await $.turn.complete(failed)
    await clock.advance(60_000)
    expect(seen.posts.map(p => p.body.limited)).toEqual([false, true])
    expect(seen.status).toEqual([])
    expect(seen.prompts).toEqual([])
  })

  test('an ordinary error or a subagent limit starts nothing', async ($, on) => {
    const { seen } = world(on, 'cli', () => null)
    await $.session.measure(measure(60))
    await $.turn.complete(failed)
    await $.session.measure(measure(100))
    await $.turn.complete({ ...failed, agentId: 'sub' })
    expect(seen.posts.every(p => !p.body.limited)).toBe(true)
  })
})

import { describe, expect, it } from 'vitest'
import { defaultTarget, plateFor, pollJob, POLL_MAX_MS, POLL_MS, stationId, type LabelJob, type Plate } from './labelJob'

const job = (over: Partial<LabelJob>): LabelJob => ({
  id: 'j1',
  status: 'queued',
  error: null,
  copies: 3,
  created_by_name: 'Админ',
  what: 'Фильтр',
  created_at: '2026-11-02T10:00:00Z',
  done_at: null,
  station_online: true,
  ...over,
})

describe('плашка задания на складе', () => {
  it('переходы: в очереди → печатается → напечатано', () => {
    expect(plateFor(job({ status: 'queued' }), 0)).toMatchObject({ kind: 'queued', final: false })
    expect(plateFor(job({ status: 'printing' }), 2_000)).toMatchObject({ kind: 'printing', final: false })
    expect(plateFor(job({ status: 'done', copies: 3 }), 4_000)).toEqual({ kind: 'done', text: 'Напечатано (3 этикетки)', final: true })
    expect(plateFor(job({ status: 'done', copies: 1 }), 0).text).toBe('Напечатано (1 этикетка)')
    expect(plateFor(job({ status: 'done', copies: 12 }), 0).text).toBe('Напечатано (12 этикеток)')
  })

  it('ошибка — текст ошибки, итог', () => {
    expect(plateFor(job({ status: 'failed', error: 'нет бумаги' }), 0)).toEqual({ kind: 'failed', text: 'Не напечатано: нет бумаги', final: true })
  })

  it('станция не отмечалась — «Склад не в сети», задание остаётся', () => {
    const p = plateFor(job({ status: 'queued', station_online: false }), 0)
    expect(p.kind).toBe('offline')
    expect(p.text).toContain('Склад не в сети')
    expect(p.final).toBe(false)
    // Уже взятое задание станция допечатает — не «не в сети».
    expect(plateFor(job({ status: 'printing', station_online: false }), 0).kind).toBe('printing')
  })

  it('по умолчанию: телефон — из настройки, компьютер — «Здесь»', () => {
    expect(defaultTarget(undefined, true)).toBe('station')
    expect(defaultTarget('station', true)).toBe('station')
    expect(defaultTarget('here', true)).toBe('here')
    expect(defaultTarget('station', false)).toBe('here')
  })

  it('номер станции хранится между запусками', () => {
    const m = new Map<string, string>()
    const st = { getItem: (k: string) => m.get(k) ?? null, setItem: (k: string, v: string) => void m.set(k, v) }
    const a = stationId(st, () => 'a')
    expect(stationId(st, () => 'b')).toBe(a)
  })
})

describe('опрос статуса', () => {
  const clock = () => {
    let t = 0
    return { now: () => t, sleep: async (ms: number) => void (t += ms) }
  }

  it('останавливается на итоге', async () => {
    const c = clock()
    const answers: LabelJob['status'][] = ['queued', 'printing', 'done']
    let calls = 0
    const seen: Plate[] = []
    const last = await pollJob('j1', {
      fetchJob: async () => job({ status: answers[Math.min(calls++, answers.length - 1)] }),
      onPlate: (p) => seen.push(p),
      ...c,
    })
    expect(calls).toBe(3)
    expect(seen.map((p) => p.kind)).toEqual(['queued', 'printing', 'done'])
    expect(last?.kind).toBe('done')
    expect(c.now()).toBe(2 * POLL_MS)
  })

  it('не дольше 2 минут', async () => {
    const c = clock()
    let calls = 0
    const last = await pollJob('j1', { fetchJob: async () => (calls++, job({ status: 'queued' })), onPlate: () => undefined, ...c })
    expect(last?.final).toBe(true)
    expect(last?.kind).toBe('timeout')
    expect(calls).toBe(POLL_MAX_MS / POLL_MS + 1)
  })

  it('ошибка сети не прерывает опрос; закрытая плашка — останавливает', async () => {
    const c = clock()
    let calls = 0
    const last = await pollJob('j1', {
      fetchJob: async () => {
        calls++
        if (calls === 1) throw new Error('нет связи')
        return job({ status: 'done' })
      },
      onPlate: () => undefined,
      ...c,
    })
    expect(last?.kind).toBe('done')
    let stop = false
    let n = 0
    await pollJob('j1', {
      fetchJob: async () => {
        n++
        stop = true
        return job({ status: 'queued' })
      },
      onPlate: () => undefined,
      stopped: () => stop,
      ...c,
    })
    expect(n).toBe(1)
  })
})

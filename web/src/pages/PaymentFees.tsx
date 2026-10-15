// Комиссия банка по способам оплаты: ставка с даты и история (SPEC-08, ADR-065). Только владелец.
import { useState } from 'react'
import { Badge, Button, Card, CardTitle, Empty, ErrorBox, Field, Loading, Modal, PageHeader, Table, toast } from '../components/ui'
import { get, newOpId, post } from '../lib/api'
import { formatDate, formatPercentBp, parsePercentBp, todayBishkek } from '../lib/format'
import { useAction, useLoad } from '../lib/hooks'

interface FeeRow {
  id: string
  method: 'card' | 'transfer'
  rate_bp: number
  active_from: string
  active_to: string | null
  user_name: string | null
  created_at: string
}

interface FeesOut {
  current: FeeRow[]
  history: FeeRow[]
}

const METHOD_LABELS: Record<FeeRow['method'], string> = { card: 'Карта', transfer: 'Перевод / QR' }

/** «с 01.01.2000» у ставки, заведённой при установке, не несёт смысла — пишем «с начала». */
const since = (d: string) => (d <= '2000-01-01' ? 'с начала учёта' : `с ${formatDate(d)}`)

export default function PaymentFees() {
  const data = useLoad(() => get<FeesOut>('/settings/payment-fees'), [])
  const [form, setForm] = useState<{ method: FeeRow['method']; rate: string; from: string; opId: string } | null>(null)
  const act = useAction()
  const bp = form ? parsePercentBp(form.rate) : null

  const open = (method: FeeRow['method'], rate: number) =>
    setForm({ method, rate: formatPercentBp(rate).replace(/ %$/, ''), from: todayBishkek(), opId: newOpId() })

  const save = () =>
    void act.run(async () => {
      if (!form || bp === null) return
      const out = await post<FeesOut>('/settings/payment-fees', { op_id: form.opId, method: form.method, rate_bp: bp, active_from: form.from })
      data.setData(out)
      setForm(null)
      toast('Ставка сохранена')
    })

  return (
    <div>
      <PageHeader title="Комиссия банка" subtitle="Сколько банк берёт с оплаты картой и переводом. Ставка фиксируется в каждом чеке." />
      <ErrorBox error={data.error} />
      {data.loading && !data.data ? (
        <Loading />
      ) : data.data ? (
        <div className="flex flex-col gap-4">
          <Card>
            <CardTitle>Действует сегодня</CardTitle>
            <div className="grid gap-3 sm:grid-cols-2">
              {data.data.current.map((f) => (
                <div key={f.method} className="flex items-center justify-between gap-3 rounded-md border border-slate-200 p-3">
                  <div>
                    <div className="text-sm text-slate-500">{METHOD_LABELS[f.method]}</div>
                    <div className="text-2xl font-bold tabular-nums">{formatPercentBp(f.rate_bp)}</div>
                    <div className="text-xs text-slate-500">{since(f.active_from)}</div>
                  </div>
                  <Button variant="secondary" onClick={() => open(f.method, f.rate_bp)}>
                    Изменить
                  </Button>
                </div>
              ))}
            </div>
            <p className="mt-3 text-xs text-slate-500">
              Новая ставка действует с выбранной даты. Чеки и погашения получают ставку своего учётного дня: чек без сети за вчера — вчерашнюю.
              Проведённые чеки свою комиссию не меняют.
            </p>
          </Card>

          <Card>
            <CardTitle>История ставок</CardTitle>
            {data.data.history.length === 0 ? (
              <Empty>Ставок пока нет</Empty>
            ) : (
              <Table head={['Способ', 'Ставка', 'Действовала', 'Кто поставил']}>
                {data.data.history.map((f) => (
                  <tr key={f.id} className={f.active_to === null ? '' : 'text-slate-500'}>
                    <td className="px-2 py-2">{METHOD_LABELS[f.method]}</td>
                    <td className="px-2 py-2 tabular-nums">{formatPercentBp(f.rate_bp)}</td>
                    <td className="px-2 py-2">
                      {since(f.active_from)}
                      {f.active_to ? ` по ${formatDate(f.active_to)} (не включая)` : ''}{' '}
                      {f.active_to === null && <Badge tone="green">действует</Badge>}
                    </td>
                    <td className="px-2 py-2">{f.user_name ?? 'при установке'}</td>
                  </tr>
                ))}
              </Table>
            )}
          </Card>
        </div>
      ) : null}

      {form && (
        <Modal title={`${METHOD_LABELS[form.method]}: новая ставка`} onClose={() => setForm(null)}>
          <div className="grid gap-3 sm:grid-cols-2">
            <Field label="Ставка, %" required hint="До двух знаков: 0,5 или 1,25">
              <input autoFocus inputMode="decimal" value={form.rate} onChange={(e) => setForm({ ...form, rate: e.target.value })} />
            </Field>
            <Field label="Действует с" required>
              <input type="date" value={form.from} onChange={(e) => setForm({ ...form, from: e.target.value })} />
            </Field>
          </div>
          {bp === null && form.rate.trim() !== '' && <div className="mt-2 text-sm text-rose-700">Ставка — число до двух знаков после запятой</div>}
          <ErrorBox error={act.error} />
          <div className="mt-3 flex justify-end gap-2">
            <Button variant="secondary" onClick={() => setForm(null)}>
              Отмена
            </Button>
            <Button disabled={act.busy || bp === null || bp > 10_000 || !form.from} onClick={save}>
              Сохранить
            </Button>
          </div>
        </Modal>
      )}
    </div>
  )
}

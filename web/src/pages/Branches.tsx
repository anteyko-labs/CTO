// «Настройки → Филиалы» (SPEC-33 §1–2): список филиалов, новый, правка, закрытие и снова открыть;
// «Расчёты между филиалами» — кто кому должен за долги клиентов, погашенные в чужом филиале.
// Только владелец.
import { useState, type FormEvent } from 'react'
import { Badge, Button, Card, CardTitle, Empty, ErrorBox, Field, Loading, Modal, Money, PageHeader, RowMenu, Table, toast } from '../components/ui'
import { get, newOpId, patch, post } from '../lib/api'
import { switchBranch } from '../lib/branch'
import { formatDate } from '../lib/format'
import { useAction, useLoad } from '../lib/hooks'
import { useUser } from '../lib/auth'
import type { Branch } from '../lib/types'

interface SettlementPair {
  branch_id: string
  branch_name: string
  creditor_branch_id: string
  creditor_name: string
  amount_tyiyn: number
}

interface SettlementEntry {
  id: string
  created_at: string
  business_date: string
  branch_name: string
  creditor_name: string
  party_name: string
  amount_tyiyn: number
  reversal: boolean
  user_name: string
}

type Form = { id?: string; name: string; address: string; phone: string }

export default function Branches() {
  const user = useUser()
  const list = useLoad(() => get<Branch[]>('/branches'), [])
  const settlements = useLoad(() => get<{ items: SettlementPair[]; entries: SettlementEntry[] }>('/branches/settlements'), [])
  const [form, setForm] = useState<Form | null>(null)
  // Операция «Новый филиал» — одна на открытое окно: повтор нажатия не заведёт второй филиал.
  const [opId, setOpId] = useState(newOpId)
  const act = useAction()

  const save = (e: FormEvent) => {
    e.preventDefault()
    if (!form) return
    void act.run(async () => {
      if (form.id) {
        await patch<Branch>(`/branches/${form.id}`, { name: form.name, address: form.address, phone: form.phone })
        toast('Филиал сохранён')
      } else {
        await post<Branch>('/branches', { op_id: opId, name: form.name, address: form.address, phone: form.phone })
        setOpId(newOpId())
        toast('Филиал заведён: кассы, счета и настройки — как в главном')
      }
      setForm(null)
      list.reload()
    })
  }

  const setArchived = (b: Branch, archived: boolean) =>
    void act.run(async () => {
      await patch<Branch>(`/branches/${b.id}`, { archived })
      list.reload()
      toast(archived ? 'Филиал закрыт: история осталась, продаж больше нет' : 'Филиал снова открыт')
    })

  return (
    <div>
      <PageHeader
        title="Филиалы"
        subtitle="Склад, деньги, смены, сотрудники и документы — у каждого филиала свои; товары, клиенты, услуги и поставщики — общие"
        actions={<Button onClick={() => setForm({ name: '', address: '', phone: '' })}>Новый филиал</Button>}
      />
      <Card>
        <ErrorBox error={list.error ?? act.error} />
        {list.loading && !list.data ? (
          <Loading />
        ) : !list.data?.length ? (
          <Empty>Филиалов нет</Empty>
        ) : (
          <Table head={['Филиал', 'Адрес', 'Телефон', 'Статус', '']}>
            {list.data.map((b) => (
              <tr key={b.id} className={b.archived ? 'text-slate-400' : ''} data-testid="branch-row">
                <td className="px-2 py-2 font-medium">
                  {b.name}
                  {b.id === user.branch_id && <span className="ml-2 text-xs text-sky-700">выбран</span>}
                </td>
                <td className="px-2 py-2">{b.address || '—'}</td>
                <td className="px-2 py-2">{b.phone || '—'}</td>
                <td className="px-2 py-2">
                  {b.archived ? <Badge>закрыт</Badge> : b.is_main ? <Badge tone="sky">главный</Badge> : <Badge tone="green">работает</Badge>}
                </td>
                <td className="overflow-visible! px-2 py-1 text-right">
                  <RowMenu
                    items={[
                      ...(b.id !== user.branch_id ? [{ label: 'Перейти в филиал', onClick: () => switchBranch(b.id) }] : []),
                      { label: 'Изменить', onClick: () => setForm({ id: b.id, name: b.name, address: b.address, phone: b.phone }) },
                      ...(b.is_main
                        ? []
                        : b.archived
                          ? [{ label: 'Открыть снова', onClick: () => setArchived(b, false) }]
                          : [{ label: 'Закрыть филиал', danger: true, onClick: () => setArchived(b, true) }]),
                    ]}
                  />
                </td>
              </tr>
            ))}
          </Table>
        )}
        <p className="mt-3 text-xs text-slate-500">
          Новый филиал получает свои «Кассу», «Счёт» и сейфы, статьи расходов и копию настроек главного филиала; номера чеков и
          документов — свои, с первого. Закрытый филиал не продаёт и не проводит документы, история и отчёты остаются. Сотрудников
          и администраторов филиала заводят, выбрав его в шапке.
        </p>
      </Card>

      <Card className="mt-4" data-testid="settlements">
        <CardTitle>Расчёты между филиалами</CardTitle>
        <ErrorBox error={settlements.error} />
        {settlements.loading && !settlements.data ? (
          <Loading />
        ) : !settlements.data?.items.length ? (
          <Empty>Филиалы друг другу ничего не должны</Empty>
        ) : (
          <Table head={['Кто должен', 'Кому', 'Сумма']}>
            {settlements.data.items.map((s) => (
              <tr key={`${s.branch_id}-${s.creditor_branch_id}`}>
                <td className="px-2 py-2">{s.branch_name}</td>
                <td className="px-2 py-2">{s.creditor_name}</td>
                <td className="px-2 py-2 text-right">
                  <Money value={s.amount_tyiyn} />
                </td>
              </tr>
            ))}
          </Table>
        )}
        {!!settlements.data?.entries.length && (
          <details className="mt-3 text-sm">
            <summary className="cursor-pointer text-sky-700">Записи</summary>
            <Table head={['День', 'Клиент', 'Принял', 'Чей долг', 'Сумма', 'Кто']}>
              {settlements.data.entries.map((e) => (
                <tr key={e.id} className={e.reversal ? 'text-slate-400' : ''}>
                  <td className="px-2 py-2">{formatDate(e.business_date)}</td>
                  <td className="px-2 py-2">{e.party_name}</td>
                  <td className="px-2 py-2">{e.branch_name}</td>
                  <td className="px-2 py-2">{e.creditor_name}</td>
                  <td className="px-2 py-2 text-right">
                    <Money value={e.amount_tyiyn} />
                    {e.reversal && ' (сторно)'}
                  </td>
                  <td className="px-2 py-2">{e.user_name}</td>
                </tr>
              ))}
            </Table>
          </details>
        )}
        <p className="mt-3 text-xs text-slate-500">
          Клиент погасил в одном филиале долг, взятый в другом: деньги лежат в кассе принявшего филиала, долг закрыт у того, где брали.
          Здесь видно, сколько один филиал должен другому.
        </p>
      </Card>

      {form && (
        <Modal title={form.id ? 'Филиал' : 'Новый филиал'} onClose={() => setForm(null)}>
          <form onSubmit={save} className="flex flex-col gap-3">
            <Field label="Название" required>
              <input autoFocus maxLength={100} value={form.name} onChange={(e) => setForm({ ...form, name: e.target.value })} />
            </Field>
            <Field label="Адрес">
              <input maxLength={300} value={form.address} onChange={(e) => setForm({ ...form, address: e.target.value })} />
            </Field>
            <Field label="Телефон">
              <input maxLength={50} inputMode="tel" value={form.phone} onChange={(e) => setForm({ ...form, phone: e.target.value })} />
            </Field>
            <ErrorBox error={act.error} />
            <div className="flex justify-end gap-2">
              <Button variant="secondary" onClick={() => setForm(null)}>
                Отмена
              </Button>
              <Button type="submit" disabled={act.busy || !form.name.trim()}>
                {form.id ? 'Сохранить' : 'Завести филиал'}
              </Button>
            </div>
          </form>
        </Modal>
      )}
    </div>
  )
}

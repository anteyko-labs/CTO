// Справочник сотрудников: кассиры и мастера без входа в систему (SPEC-02, инвариант 15).
// Владелец меняет в одной карточке всё: ФИО, роли, телефон, активность и правила оплаты (SPEC-27).
// Карточка сотрудника у владельца — ещё и правила оплаты с историей (SPEC-07) и телефон, по которому
// работник видит свой заработок в Телеграм-боте (SPEC-23).
import { useState, type FormEvent } from 'react'
import { Link } from 'react-router-dom'
import { Icon } from '../components/icons'
import { Badge, Button, Card, Checkbox, Empty, ErrorBox, Field, Loading, Missing, Modal, PageHeader, RowMenu, Table, toast } from '../components/ui'
import { del, get, newOpId, patch, post } from '../lib/api'
import { useUser } from '../lib/auth'
import { formatDate, formatPercentBp, formatSom, parsePercentBp, parseSom } from '../lib/format'
import { missingWithFocus } from '../lib/forms'
import { useAction, useLoad } from '../lib/hooks'
import type { Employee } from '../lib/types'

type RuleKind = 'revenue_percent' | 'monthly_salary' | 'per_shift'

interface PayRule {
  id: string
  kind: RuleKind | 'per_service'
  role: string
  base: string | null
  amount_tyiyn: number | null
  rate_bp: number | null
  active_from: string
  active_to: string | null
}

const RULE_LABELS: Record<PayRule['kind'], string> = {
  revenue_percent: '% с валовой прибыли чеков',
  monthly_salary: 'Оклад в месяц',
  per_shift: 'За смену',
  per_service: 'За замену (старое правило)',
}

const ruleValue = (r: PayRule) => (r.kind === 'revenue_percent' ? formatPercentBp(r.rate_bp ?? 0) : formatSom(r.amount_tyiyn ?? 0))

/** Правила оплаты сотрудника: действующие, история и новое правило (SPEC-07). Только владелец.
 *  Новое правило того же вида закрывает прежнее сегодняшним днём; начисленное не меняется. */
function PayRules({ employee }: { employee: Employee }) {
  const rules = useLoad(() => get<PayRule[]>(`/employees/${employee.id}/pay-rules`), [employee.id])
  const [form, setForm] = useState<{ kind: RuleKind; value: string } | null>(null)
  // op_id формы: повтор после потерянного ответа не заведёт второе правило, после успеха — новый.
  const [opId, setOpId] = useState(newOpId)
  const act = useAction()
  const active = (rules.data ?? []).filter((r) => r.active_to === null)
  const closed = (rules.data ?? []).filter((r) => r.active_to !== null)
  const parsed = form ? (form.kind === 'revenue_percent' ? parsePercentBp(form.value) : parseSom(form.value)) : null

  const add = () =>
    void act.run(async () => {
      if (!form || parsed === null || parsed <= 0) throw new Error('Неверное значение')
      await post(`/employees/${employee.id}/pay-rules`, {
        op_id: opId,
        kind: form.kind,
        role: 'cashier',
        ...(form.kind === 'revenue_percent' ? { rate_bp: parsed, base: 'gross' } : { amount_tyiyn: parsed }),
      })
      setForm(null)
      setOpId(newOpId())
      rules.reload()
      toast('Правило оплаты сохранено')
    })

  const close = (r: PayRule) =>
    void act.run(async () => {
      if (!window.confirm(`Закрыть правило «${RULE_LABELS[r.kind]} ${ruleValue(r)}» с сегодняшнего дня? Начисленное раньше не изменится.`)) return
      await del(`/employees/${employee.id}/pay-rules/${r.id}`)
      rules.reload()
      toast('Правило закрыто')
    })

  return (
    <div className="flex flex-col gap-2 border-t border-slate-200 pt-3">
      <div className="flex items-center justify-between gap-2">
        <h3 className="font-semibold">Оплата</h3>
        {!form && (
          <Button variant="secondary" className="px-2 py-1 text-xs" onClick={() => setForm({ kind: 'revenue_percent', value: '' })}>
            Новое правило
          </Button>
        )}
      </div>
      <ErrorBox error={rules.error ?? act.error} />
      {rules.loading && !rules.data ? (
        <Loading />
      ) : active.length === 0 ? (
        <div className="text-sm text-slate-500">Действующих правил нет — сотрудник получает только то, что начислено вручную.</div>
      ) : (
        <ul className="flex flex-col gap-1 text-sm">
          {active.map((r) => (
            <li key={r.id} className="flex flex-wrap items-center justify-between gap-2">
              <span>
                {RULE_LABELS[r.kind]}: <b className="tabular-nums">{ruleValue(r)}</b>
                <span className="text-xs text-slate-500"> · с {formatDate(r.active_from)}</span>
              </span>
              <Button variant="secondary" className="px-2 py-1 text-xs" disabled={act.busy} onClick={() => close(r)}>
                Закрыть
              </Button>
            </li>
          ))}
        </ul>
      )}
      {employee.is_master && (
        <div className="text-xs text-slate-500">
          Мастеру за замену платится ставка из чека — она общая для точки и меняется на экране{' '}
          <Link to="/services" className="underline">
            «Услуги»
          </Link>
          .
        </div>
      )}
      {form && (
        <div className="grid gap-2 rounded-md border border-slate-200 p-3 sm:grid-cols-[1fr_10rem_auto] sm:items-end">
          <Field label="Вид">
            <select value={form.kind} onChange={(e) => setForm({ kind: e.target.value as RuleKind, value: '' })}>
              <option value="revenue_percent">{RULE_LABELS.revenue_percent}</option>
              <option value="monthly_salary">{RULE_LABELS.monthly_salary}</option>
              <option value="per_shift">{RULE_LABELS.per_shift}</option>
            </select>
          </Field>
          <Field label={form.kind === 'revenue_percent' ? 'Процент, %' : 'Сумма, с'} required>
            <input autoFocus inputMode="decimal" value={form.value} onChange={(e) => setForm({ ...form, value: e.target.value })} />
          </Field>
          <div className="flex gap-2">
            <Button variant="secondary" onClick={() => setForm(null)}>
              Отмена
            </Button>
            <Button disabled={act.busy || parsed === null || parsed <= 0} onClick={add}>
              Сохранить
            </Button>
          </div>
          <div className="text-xs text-slate-500 sm:col-span-3">
            {form.kind === 'revenue_percent'
              ? 'Каждый день с валовой прибыли чеков кассира за вычетом скидки баллами.'
              : form.kind === 'monthly_salary'
                ? 'Начисляется кнопкой «Оклад» в расчёте за месяц; сумму можно поправить.'
                : 'Начисляется кассиру смены при её закрытии.'}{' '}
            Прежнее правило того же вида закроется сегодняшним днём.
          </div>
        </div>
      )}
      {closed.length > 0 && (
        <details className="text-sm">
          <summary className="cursor-pointer text-slate-600">История правил ({closed.length})</summary>
          <ul className="mt-1 flex flex-col gap-1 text-slate-500">
            {closed.map((r) => (
              <li key={r.id}>
                {RULE_LABELS[r.kind]}: {ruleValue(r)} · с {formatDate(r.active_from)} по {r.active_to ? formatDate(r.active_to) : '—'}
              </li>
            ))}
          </ul>
        </details>
      )}
    </div>
  )
}

/** Роли сотрудника значками: «кассир», «мастер»; у кассира без PIN — «без PIN» (ADR-081). */
function Roles({ e }: { e: Employee }) {
  if (!e.is_cashier && !e.is_master) return <span className="text-slate-400">—</span>
  return (
    <span className="flex flex-wrap gap-1">
      {e.is_cashier && <Badge tone="sky">кассир</Badge>}
      {e.is_cashier && e.has_pin === false && <Badge tone="rose">без PIN</Badge>}
      {e.is_master && <Badge tone="amber">мастер</Badge>}
    </span>
  )
}

/** PIN кассира — 4 цифры (ADR-081). */
const pinOk = (pin: string): boolean => /^\d{4}$/.test(pin)

/** PIN кассира: задаёт, меняет и снимает только владелец; сам PIN нигде не показывается (ADR-081).
 *  Без PIN смену на кассира откроет только владелец своим паролем. */
function PinBox({ employee, pin, setPin }: { employee: Employee; pin: string; setPin: (v: string) => void }) {
  const [has, setHas] = useState(Boolean(employee.has_pin))
  const act = useAction()
  const save = (value: string | null) =>
    void act.run(async () => {
      const e = await post<Employee>(`/employees/${employee.id}/pin`, { pin: value })
      setHas(Boolean(e.has_pin))
      setPin('')
      toast(value ? 'PIN сохранён' : 'PIN снят')
    })
  return (
    <div className="flex flex-col gap-2 border-t border-slate-200 pt-3" data-testid="pin-box">
      <div className="flex items-center justify-between gap-2">
        <h3 className="font-semibold">PIN кассира</h3>
        {has ? <Badge tone="green">задан</Badge> : <Badge tone="rose">не задан</Badge>}
      </div>
      <div className="text-xs text-slate-500">
        Кассир вводит PIN при открытии смены. 5 неверных подряд — блокировка на 15 минут; новый PIN снимает её.
      </div>
      <div className="flex flex-wrap items-end gap-2">
        <Field label={has ? 'Новый PIN' : 'PIN, 4 цифры'}>
          <input
            id="employee-pin"
            className="w-28 text-center tracking-[0.4em]"
            type="password"
            inputMode="numeric"
            autoComplete="new-password"
            maxLength={4}
            value={pin}
            onChange={(e) => setPin(e.target.value.replace(/\D/g, '').slice(0, 4))}
          />
        </Field>
        <Button disabled={act.busy || !pinOk(pin)} onClick={() => save(pin)}>
          {has ? 'Сменить PIN' : 'Задать PIN'}
        </Button>
        {has && (
          <Button variant="ghost" disabled={act.busy} onClick={() => save(null)}>
            Снять
          </Button>
        )}
      </div>
      <ErrorBox error={act.error} />
    </div>
  )
}

/** Телефон для бота: пусто или не меньше 9 цифр — сверяются последние 9, как у клиентов. */
const phoneOk = (phone: string) => {
  const t = phone.trim()
  return t === '' || t.replace(/\D/g, '').length >= 9
}

/** Признак «кассир» назначает оплату (2 % и оклад) — ставит только владелец; сервер проверяет сам (ADR-059).
 *  Телефон открывает работнику его начисления в боте — его тоже задаёт только владелец (SPEC-23). */
function EditModal({ employee, onClose, onSaved }: { employee: Employee; onClose: () => void; onSaved: () => void }) {
  const owner = useUser().role === 'owner'
  const [form, setForm] = useState({ ...employee, phone: employee.phone ?? '' })
  // Вписанный, но не сохранённый PIN общая «Сохранить» не теряет молча (проверка 20.10.2026):
  // верный — сохраняется вместе с карточкой, неполный — ошибка.
  const [pin, setPin] = useState('')
  const { busy, error, run } = useAction()

  const save = () =>
    run(async () => {
      if (pin !== '' && !pinOk(pin)) throw new Error('PIN кассира — 4 цифры: допишите или очистите поле')
      if (pin !== '') await post<Employee>(`/employees/${employee.id}/pin`, { pin })
      await patch<Employee>(`/employees/${employee.id}`, {
        full_name: form.full_name,
        is_cashier: form.is_cashier,
        is_master: form.is_master,
        active: form.active,
        ...(owner ? { phone: form.phone.trim() } : {}),
      })
      toast('Сотрудник сохранён')
      onSaved()
    })

  return (
    <Modal title="Сотрудник" onClose={onClose}>
      <div className="flex flex-col gap-3">
        <Field label="ФИО">
          <input value={form.full_name} onChange={(e) => setForm({ ...form, full_name: e.target.value })} />
        </Field>
        {owner && (
          <Field label="Телефон для бота" error={phoneOk(form.phone) ? undefined : 'Не меньше 9 цифр'}>
            <input
              type="tel"
              inputMode="tel"
              placeholder="+996 555 12 34 56"
              value={form.phone}
              onChange={(e) => setForm({ ...form, phone: e.target.value })}
            />
          </Field>
        )}
        {owner && (
          <p className="-mt-2 text-xs text-slate-500">
            По этому номеру работник подключается к Телеграм-боту: там его заработок, удержания и премии. Сменили номер или
            отключили сотрудника — бот отключает его сразу.
          </p>
        )}
        <div className="flex flex-wrap gap-4">
          {owner && <Checkbox label="Кассир" checked={form.is_cashier} onChange={(v) => setForm({ ...form, is_cashier: v })} />}
          <Checkbox label="Мастер" checked={form.is_master} onChange={(v) => setForm({ ...form, is_master: v })} />
          <Checkbox label="Активен" checked={form.active} onChange={(v) => setForm({ ...form, active: v })} />
        </div>
        <ErrorBox error={error} />
        <div className="flex justify-end gap-2">
          <Button variant="secondary" onClick={onClose}>
            Отмена
          </Button>
          <Button disabled={busy || !form.full_name.trim() || !phoneOk(form.phone)} onClick={() => void save()}>
            Сохранить
          </Button>
        </div>
        {owner && employee.is_cashier && <PinBox employee={employee} pin={pin} setPin={setPin} />}
        {owner && <PayRules employee={employee} />}
      </div>
    </Modal>
  )
}

export default function Employees() {
  const owner = useUser().role === 'owner'
  const list = useLoad(() => get<Employee[]>('/employees'), [])
  const [form, setForm] = useState({ full_name: '', is_cashier: false, is_master: true, phone: '', pin: '' })
  const [adding, setAdding] = useState(false)
  const [q, setQ] = useState('')
  const [editing, setEditing] = useState<Employee | null>(null)
  const { busy, error, run } = useAction()
  const toggle = useAction()

  const create = (e: FormEvent) => {
    e.preventDefault()
    void run(async () => {
      await post<Employee>('/employees', {
        full_name: form.full_name,
        is_master: form.is_master,
        is_cashier: owner && form.is_cashier,
        ...(owner ? { phone: form.phone.trim() } : {}),
        ...(owner && form.is_cashier && form.pin ? { pin: form.pin } : {}),
      })
      setForm({ ...form, full_name: '', phone: '', pin: '' })
      setAdding(false)
      list.reload()
    })
  }

  const setActive = (e: Employee, active: boolean) =>
    void toggle.run(async () => {
      await patch<Employee>(`/employees/${e.id}`, { full_name: e.full_name, is_cashier: e.is_cashier, is_master: e.is_master, active })
      toast(active ? 'Сотрудник включён' : 'Сотрудник отключён')
      list.reload()
    })

  const notFilled = missingWithFocus(
    [Boolean(form.full_name.trim()), 'ФИО', '#employee-name'],
    [form.is_cashier || form.is_master, owner ? 'роль: кассир или мастер' : 'роль: мастер'],
    [phoneOk(form.phone), 'телефон: не меньше 9 цифр', '#employee-phone'],
    [!form.is_cashier || form.pin === '' || pinOk(form.pin), 'PIN кассира: 4 цифры', '#employee-new-pin'],
  )
  const needle = q.trim().toLowerCase()
  const rows = (list.data ?? []).filter((e) => !needle || e.full_name.toLowerCase().includes(needle))

  return (
    <div>
      <PageHeader
        title="Сотрудники"
        actions={
          <Button onClick={() => setAdding(true)}>
            <Icon name="plus" /> Добавить
          </Button>
        }
      />

      <Card>
        {(list.data?.length ?? 0) > 5 && (
          <div className="mb-3">
            <input aria-label="Поиск сотрудника" placeholder="Поиск по ФИО" value={q} onChange={(e) => setQ(e.target.value)} />
          </div>
        )}
        <ErrorBox error={list.error ?? toggle.error} />
        {list.loading && !list.data ? (
          <Loading />
        ) : !list.data?.length ? (
          <Empty
            icon="employees"
            action={
              <Button onClick={() => setAdding(true)}>
                <Icon name="plus" /> Добавить сотрудника
              </Button>
            }
          >
            Сотрудников пока нет. Заведите кассира и мастера — их выбирают в чеке.
          </Empty>
        ) : rows.length === 0 ? (
          <Empty>Никого не найдено</Empty>
        ) : (
          <Table head={['ФИО', 'Роли', 'Статус', '']}>
            {rows.map((e) => (
              <tr
                key={e.id}
                className={`cursor-pointer hover:bg-slate-50 ${e.active ? '' : 'text-slate-400'}`}
                onClick={() => setEditing(e)}
              >
                <td className="px-2 py-2 font-medium">{e.full_name}</td>
                <td className="px-2 py-2">
                  <Roles e={e} />
                </td>
                <td className="px-2 py-2">{e.active ? <Badge tone="green">активен</Badge> : <Badge>отключён</Badge>}</td>
                <td className="overflow-visible! px-2 py-1 text-right" onClick={(ev) => ev.stopPropagation()}>
                  <RowMenu
                    items={[
                      { label: 'Изменить', onClick: () => setEditing(e) },
                      e.active
                        ? { label: 'Отключить', danger: true, onClick: () => setActive(e, false) }
                        : { label: 'Включить', onClick: () => setActive(e, true) },
                    ]}
                  />
                </td>
              </tr>
            ))}
          </Table>
        )}
      </Card>

      {adding && (
        <Modal title="Новый сотрудник" onClose={() => setAdding(false)}>
          <form onSubmit={create} className="flex flex-col gap-3">
            <Field label="ФИО" required>
              <input id="employee-name" autoFocus value={form.full_name} onChange={(e) => setForm({ ...form, full_name: e.target.value })} />
            </Field>
            {owner && (
              <Field label="Телефон для бота" hint="Можно позже, в карточке сотрудника">
                <input
                  id="employee-phone"
                  type="tel"
                  inputMode="tel"
                  placeholder="+996 555 12 34 56"
                  value={form.phone}
                  onChange={(e) => setForm({ ...form, phone: e.target.value })}
                />
              </Field>
            )}
            <div className="flex flex-wrap gap-4">
              {owner && <Checkbox label="Кассир" checked={form.is_cashier} onChange={(v) => setForm({ ...form, is_cashier: v })} />}
              <Checkbox label="Мастер" checked={form.is_master} onChange={(v) => setForm({ ...form, is_master: v })} />
            </div>
            {owner && form.is_cashier && (
              <Field label="PIN кассира, 4 цифры" hint="Вводится при открытии смены. Можно задать позже в карточке">
                <input
                  id="employee-new-pin"
                  className="w-28 text-center tracking-[0.4em]"
                  type="password"
                  inputMode="numeric"
                  autoComplete="new-password"
                  maxLength={4}
                  value={form.pin}
                  onChange={(e) => setForm({ ...form, pin: e.target.value.replace(/\D/g, '').slice(0, 4) })}
                />
              </Field>
            )}
            {!owner && <p className="text-xs text-slate-500">Кассира заводит владелец: вместе с ним назначается оплата.</p>}
            <ErrorBox error={error} />
            <div className="flex flex-wrap items-center justify-end gap-2">
              <Missing items={notFilled} className="mr-auto" />
              <Button variant="secondary" onClick={() => setAdding(false)}>
                Отмена
              </Button>
              <Button type="submit" disabled={busy || notFilled.length > 0}>
                Добавить
              </Button>
            </div>
          </form>
        </Modal>
      )}

      {editing && (
        <EditModal
          employee={editing}
          onClose={() => {
            // PIN мог смениться в карточке — значок «без PIN» в списке обновится.
            setEditing(null)
            list.reload()
          }}
          onSaved={() => {
            setEditing(null)
            list.reload()
          }}
        />
      )}
    </div>
  )
}

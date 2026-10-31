// «Настройки» → «Сообщения клиентам» (SPEC-32): WhatsApp Business Platform — шаблоны Meta и их
// переменные по порядку, пробег в день по умолчанию, «Отправить пробное» и журнал сообщений.
// «Должникам» (задача 002, П7, П7б, П9): «Получили оплату», раз в месяц — кто сколько должен,
// напоминание о сроке — выключатели, тексты с подстановками, образец и «Отправить себе».
// Только владелец.
import { useState } from 'react'
import { Link } from 'react-router-dom'
import { Pager, usePagedLoad } from '../components/Pager'
import { ExportButton } from '../components/ExportButton'
import { cappedWarning, fetchAllPages, listSheet, xl, type ExportCol } from '../lib/exportTable'
import { Badge, Button, Card, CardTitle, Checkbox, Empty, ErrorBox, Field, Loading, Notice, PageHeader, Table, toast } from '../components/ui'
import { get, newOpId, post, put, qs } from '../lib/api'
import {
  CHANNEL_LABELS,
  DEBT_DUE_MARKS,
  KIND_LABELS,
  MONTHLY_FIRM_MARKS,
  MONTHLY_MARKS,
  MONTHLY_TEMPLATE_TEXT,
  OIL_TEMPLATE_TEXT,
  PAYMENT_MARKS,
  PAYMENT_TEMPLATE_TEXT,
  PURCHASE_TEMPLATE_TEXT,
  SAMPLE_VALUES,
  STATUS_LABELS,
  languageProblem,
  paramLines,
  recipientText,
  sampleText,
  statusTone,
  templateNameProblem,
  textProblem,
  type ClientMessage,
  type MessageKind,
  type MessageSettings,
  type MessageSettingsOut,
} from '../lib/clientMessages'
import { formatDateTime } from '../lib/format'
import { useAction, useDebounced, useLoad } from '../lib/hooks'
import type { Party } from '../lib/types'

export default function ClientMessages() {
  const st = useLoad(() => get<MessageSettingsOut>('/settings/client-messages'), [])
  const [tick, setTick] = useState(0)
  if (st.loading && !st.data) return <Loading />
  if (!st.data) return <ErrorBox error={st.error} />
  return (
    <div className="flex max-w-5xl flex-col gap-4">
      <PageHeader
        title="Сообщения клиентам"
        subtitle="Напоминание о замене масла, сообщение юрлицу о закупе и сообщения должникам: в Телеграм, если клиент подключён к боту, иначе — в WhatsApp"
        back={
          <Link className="text-sky-700 hover:underline" to="/settings">
            ← Настройки
          </Link>
        }
      />
      <SettingsForm s={st.data} />
      <TestSend onSent={() => setTick((t) => t + 1)} />
      <Journal key={tick} />
    </div>
  )
}

function TemplateBox({
  title,
  text,
  params,
  name,
  lang,
  onName,
  onLang,
  id,
}: {
  title: string
  text: string
  params: string[]
  name: string
  lang: string
  onName: (v: string) => void
  onLang: (v: string) => void
  id: string
}) {
  return (
    <div className="flex flex-col gap-2 rounded-md border border-slate-200 p-3">
      <div className="font-medium">{title}</div>
      <div className="grid gap-3 sm:grid-cols-[1fr_8rem]">
        <Field label="Имя шаблона в WhatsApp Manager" hint="Пусто — в WhatsApp не отправляется" error={templateNameProblem(name)}>
          <input id={`${id}-template`} value={name} onChange={(e) => onName(e.target.value)} placeholder="avtodom_oil_change" />
        </Field>
        <Field label="Язык" error={languageProblem(lang)}>
          <input id={`${id}-language`} value={lang} onChange={(e) => onLang(e.target.value)} />
        </Field>
      </div>
      <div className="text-xs text-slate-500">Текст шаблона для Meta (категория «Служебные»):</div>
      <pre className="whitespace-pre-wrap rounded bg-slate-50 p-2 text-xs">{text}</pre>
      <ul className="text-xs text-slate-600">
        {paramLines(params).map((l) => (
          <li key={l}>{l}</li>
        ))}
      </ul>
    </div>
  )
}

/** Поле текста сообщения должнику: подстановки, образец с примером и «Отправить себе». */
function DebtorText({
  id,
  label,
  kind,
  value,
  onChange,
  marks,
  firmText,
  optional = false,
  hint,
}: {
  id: string
  label: string
  kind: 'payment' | 'monthly' | 'debt_due'
  value: string
  onChange: (v: string) => void
  marks: readonly string[]
  firmText?: string
  optional?: boolean
  hint?: string
}) {
  const send = useAction()
  const problem = textProblem(value, marks, optional)
  return (
    <div className="flex flex-col gap-1">
      <Field label={label} hint={hint ?? `Подстановки: ${marks.map((m) => `{${m}}`).join(', ')}`} error={problem}>
        <textarea id={id} rows={2} value={value} onChange={(e) => onChange(e.target.value)} />
      </Field>
      {value.trim() && (
        <div className="text-xs text-slate-500" data-testid={`${id}-sample`}>
          Образец: {sampleText(value.trim(), SAMPLE_VALUES)}
        </div>
      )}
      {!optional && (
        <div className="flex items-center gap-2">
          <Button
            variant="secondary"
            className="min-h-[34px] px-3 py-1 text-xs"
            disabled={problem !== null || send.busy}
            onClick={() =>
              void send.run(async () => {
                const r = await post<{ text: string; sent: boolean }>('/client-messages/sample', {
                  kind,
                  text: value.trim(),
                  firm_text: firmText?.trim() || null,
                })
                toast(r.sent ? 'Отправлено вам в Телеграм' : 'Телеграм не принял сообщение')
              })
            }
          >
            Отправить себе
          </Button>
          <ErrorBox error={send.error} />
        </div>
      )}
    </div>
  )
}

/** Настройка как её отправляет экран: все поля, без справочных. */
function editable(s: MessageSettingsOut): MessageSettings {
  return {
    whatsapp_enabled: s.whatsapp_enabled,
    oil_template: s.oil_template,
    oil_language: s.oil_language,
    purchase_template: s.purchase_template,
    purchase_language: s.purchase_language,
    km_per_day: s.km_per_day,
    payment_enabled: s.payment_enabled,
    payment_text: s.payment_text,
    payment_template: s.payment_template,
    payment_language: s.payment_language,
    monthly_enabled: s.monthly_enabled,
    monthly_day: s.monthly_day,
    monthly_hour: s.monthly_hour,
    monthly_text: s.monthly_text,
    monthly_firm_text: s.monthly_firm_text,
    monthly_template: s.monthly_template,
    monthly_language: s.monthly_language,
    debt_due_text: s.debt_due_text,
  }
}

function SettingsForm({ s }: { s: MessageSettingsOut }) {
  const [f, setF] = useState<MessageSettings>(() => editable(s))
  const [km, setKm] = useState(String(s.km_per_day))
  const set = <K extends keyof MessageSettings>(k: K) => (v: MessageSettings[K]) => setF((x) => ({ ...x, [k]: v }))
  const save = useAction()
  const kmValue = Number(km)
  const kmOk = Number.isInteger(kmValue) && kmValue >= 1 && kmValue <= 1000
  const dayOk = Number.isInteger(f.monthly_day) && f.monthly_day >= 1 && f.monthly_day <= 28
  const hourOk = Number.isInteger(f.monthly_hour) && f.monthly_hour >= 9 && f.monthly_hour <= 19
  const bad =
    !kmOk ||
    !dayOk ||
    !hourOk ||
    [f.oil_template, f.purchase_template, f.payment_template, f.monthly_template].some((n) => templateNameProblem(n) !== null) ||
    [f.oil_language, f.purchase_language, f.payment_language, f.monthly_language].some((l) => languageProblem(l) !== null) ||
    textProblem(f.payment_text, PAYMENT_MARKS) !== null ||
    textProblem(f.monthly_text, MONTHLY_MARKS) !== null ||
    textProblem(f.monthly_firm_text, MONTHLY_FIRM_MARKS, true) !== null ||
    textProblem(f.debt_due_text, DEBT_DUE_MARKS) !== null
  const saveButton = (
    <div>
      <ErrorBox error={save.error} />
      <Button
        disabled={bad || save.busy}
        onClick={() =>
          void save.run(async () => {
            await put('/settings/client-messages', { op_id: newOpId(), ...f, km_per_day: kmValue })
            toast('Сохранено')
          })
        }
      >
        Сохранить
      </Button>
    </div>
  )
  return (
    <>
      <Card className="flex flex-col gap-3">
        <CardTitle>Каналы и шаблоны</CardTitle>
        <div className="text-sm text-slate-600">
          Телеграм-бот: {s.telegram ? 'включён' : 'выключен (нет ключа бота на сервере)'} · WhatsApp:{' '}
          {s.whatsapp_configured ? 'ключ и номер заданы на сервере' : 'не подключён'} · Ссылка на кабинет:{' '}
          {s.public_url ? `${s.public_url}/cabinet` : 'внешний адрес не задан — вместо неё ссылка на бот'}
        </div>
        {!s.whatsapp_configured && (
          <Notice>
            WhatsApp Business Platform не подключён: на точке в <code>config.ps1</code> впишите <code>$WhatsAppToken</code> и{' '}
            <code>$WhatsAppPhoneId</code> из кабинета Meta и перезапустите сервер. Шаблоны ниже подаются в Meta на одобрение заранее.
          </Notice>
        )}
        <Checkbox label="Отправлять в WhatsApp клиентам, не подключённым к боту" checked={f.whatsapp_enabled} onChange={set('whatsapp_enabled')} />
        <div className="grid gap-3 lg:grid-cols-2">
          <TemplateBox
            id="oil"
            title="Замена масла"
            text={OIL_TEMPLATE_TEXT}
            params={s.oil_params}
            name={f.oil_template}
            lang={f.oil_language}
            onName={set('oil_template')}
            onLang={set('oil_language')}
          />
          <TemplateBox
            id="purchase"
            title="Закуп юрлица"
            text={PURCHASE_TEMPLATE_TEXT}
            params={s.purchase_params}
            name={f.purchase_template}
            lang={f.purchase_language}
            onName={set('purchase_template')}
            onLang={set('purchase_language')}
          />
          <TemplateBox
            id="payment"
            title="Получили оплату (только юрлицам)"
            text={PAYMENT_TEMPLATE_TEXT}
            params={s.payment_params}
            name={f.payment_template}
            lang={f.payment_language}
            onName={set('payment_template')}
            onLang={set('payment_language')}
          />
          <TemplateBox
            id="monthly"
            title="Долг раз в месяц (только юрлицам)"
            text={MONTHLY_TEMPLATE_TEXT}
            params={s.monthly_params}
            name={f.monthly_template}
            lang={f.monthly_language}
            onName={set('monthly_template')}
            onLang={set('monthly_language')}
          />
        </div>
        <Field
          label="Пробег в день по умолчанию, км"
          hint="Если в книжке машины одна запись с пробегом; иначе — средний по книжке"
          error={kmOk ? null : 'от 1 до 1000'}
        >
          <input id="km-per-day" className="w-28" inputMode="numeric" value={km} onChange={(e) => setKm(e.target.value)} />
        </Field>
        {saveButton}
      </Card>
      <Card className="flex flex-col gap-4" data-testid="debtor-messages">
        <CardTitle>Должникам</CardTitle>
        <div className="-mt-2 text-sm text-slate-600">
          В бот — всем подключённым клиентам; в WhatsApp — только юрлицам, по шаблону выше; юрлицу — ещё в ленту «Сообщения» его кабинета. Отправка — с 09:00 до
          20:00, ночью — утром. Одно и то же дважды не уходит.
        </div>
        <div className="flex flex-col gap-2 rounded-md border border-slate-200 p-3">
          <Checkbox label="«Получили оплату» — после каждого погашения долга" checked={f.payment_enabled} onChange={set('payment_enabled')} />
          <DebtorText id="payment-text" label="Текст для бота" kind="payment" value={f.payment_text} onChange={set('payment_text')} marks={PAYMENT_MARKS} />
        </div>
        <div className="flex flex-col gap-2 rounded-md border border-slate-200 p-3">
          <Checkbox label="Раз в месяц — кто сколько должен (каждому с долгом больше нуля)" checked={f.monthly_enabled} onChange={set('monthly_enabled')} />
          <div className="flex flex-wrap gap-3">
            <Field label="День месяца" error={dayOk ? null : 'от 1 до 28'}>
              <input
                id="monthly-day"
                className="w-20"
                inputMode="numeric"
                value={String(f.monthly_day)}
                onChange={(e) => set('monthly_day')(Number(e.target.value))}
              />
            </Field>
            <Field label="Час (по Бишкеку)" error={hourOk ? null : 'с 9 до 19'}>
              <input
                id="monthly-hour"
                className="w-20"
                inputMode="numeric"
                value={String(f.monthly_hour)}
                onChange={(e) => set('monthly_hour')(Number(e.target.value))}
              />
            </Field>
          </div>
          <DebtorText id="monthly-text" label="Текст для бота" kind="monthly" value={f.monthly_text} onChange={set('monthly_text')} marks={MONTHLY_MARKS} firmText={f.monthly_firm_text} />
          <DebtorText
            id="monthly-firm-text"
            label="Юрлицу — дописать (пусто — ничего)"
            kind="monthly"
            value={f.monthly_firm_text}
            onChange={set('monthly_firm_text')}
            marks={MONTHLY_FIRM_MARKS}
            optional
            hint="Ссылка на кабинет — только при внешнем адресе программы. Подстановки: {ссылка}, {имя}, {долг}, {дата}"
          />
        </div>
        <div className="flex flex-col gap-2 rounded-md border border-slate-200 p-3">
          <div className="font-medium">Напоминание о сроке долга (в бот за 3 дня и в день срока)</div>
          <DebtorText id="debt-due-text" label="Текст" kind="debt_due" value={f.debt_due_text} onChange={set('debt_due_text')} marks={DEBT_DUE_MARKS} />
        </div>
        {saveButton}
      </Card>
    </>
  )
}

/** «Отправить пробное»: клиенту по его каналу, сразу, без окна отправки. */
function TestSend({ onSent }: { onSent: () => void }) {
  const [q, setQ] = useState('')
  const [party, setParty] = useState<Party | null>(null)
  const [kind, setKind] = useState<MessageKind>('oil')
  const query = useDebounced(q.trim(), 300)
  const found = useLoad(
    () => (query.length < 2 ? Promise.resolve([] as Party[]) : get<Party[]>(`/parties${qs({ role: 'customer', q: query, limit: 8 })}`)),
    [query],
  )
  const send = useAction()
  const [result, setResult] = useState<ClientMessage[] | null>(null)
  return (
    <Card className="flex flex-col gap-3">
      <CardTitle>Отправить пробное</CardTitle>
      <div className="grid gap-3 sm:grid-cols-[1fr_12rem]">
        <Field label="Клиент" hint={party ? `Выбран: ${party.name}${party.phone ? `, ${party.phone}` : ''}` : 'Имя, телефон или госномер'}>
          <input
            id="test-client"
            value={q}
            onChange={(e) => {
              setQ(e.target.value)
              setParty(null)
            }}
          />
        </Field>
        <Field label="Сообщение">
          <select id="test-kind" value={kind} onChange={(e) => setKind(e.target.value as MessageKind)}>
            <option value="oil">{KIND_LABELS.oil}</option>
            <option value="purchase">{KIND_LABELS.purchase}</option>
            <option value="payment">{KIND_LABELS.payment}</option>
            <option value="monthly">{KIND_LABELS.monthly}</option>
          </select>
        </Field>
      </div>
      {!party && (found.data ?? []).length > 0 && (
        <ul className="flex flex-col rounded-md border border-slate-200 text-sm">
          {(found.data ?? []).map((p) => (
            <li key={p.id}>
              <button
                type="button"
                className="w-full px-3 py-2 text-left hover:bg-slate-50"
                onClick={() => {
                  setParty(p)
                  setQ(p.name)
                }}
              >
                {p.name} <span className="text-slate-500">{p.phone}</span>
              </button>
            </li>
          ))}
        </ul>
      )}
      <ErrorBox error={send.error} />
      {result &&
        result.map((m) => (
          <div key={m.id} className="text-sm">
            <Badge tone={statusTone(m.status)}>{STATUS_LABELS[m.status]}</Badge> {CHANNEL_LABELS[m.channel]} {recipientText(m)}
            {m.error && <span className="text-rose-700"> — {m.error}</span>}
          </div>
        ))}
      <div>
        <Button
          variant="secondary"
          disabled={!party || send.busy}
          onClick={() =>
            void send.run(async () => {
              if (!party) return
              const r = await post<ClientMessage[]>('/client-messages/test', { party_id: party.id, kind })
              setResult(r)
              onSent()
            })
          }
        >
          {send.busy ? 'Отправляем…' : 'Отправить пробное'}
        </Button>
      </div>
    </Card>
  )
}

function Journal() {
  const list = usePagedLoad<ClientMessage>('/client-messages', 'client-messages')
  return (
    <Card>
      <CardTitle
        actions={
          <div className="flex flex-wrap gap-2">
            {/* Весь журнал, а не страница (задача 003 Н13). */}
            <ExportButton
              file="Журнал сообщений"
              disabled={list.total === 0}
              sheets={async () => {
                const all = await fetchAllPages<ClientMessage>('/client-messages')
                return [listSheet('Журнал сообщений', MESSAGE_COLS, all.items, { sheet: 'Сообщения', warning: cappedWarning(all) })]
              }}
            />
            <Button variant="secondary" className="min-h-[34px] px-3 py-1 text-xs" onClick={list.reload}>
              Обновить
            </Button>
          </div>
        }
      >
        Журнал сообщений
      </CardTitle>
      <ErrorBox error={list.error} />
      {list.loading && list.items.length === 0 ? (
        <Loading />
      ) : list.items.length === 0 ? (
        <Empty>Сообщений ещё не было.</Empty>
      ) : (
        <Table head={['Когда', 'Кому', 'Вид', 'Канал', 'Текст', 'Итог']}>
          {list.items.map((m) => (
            <tr key={m.id} data-testid="client-message">
              <td className="px-2 py-2 whitespace-nowrap">{formatDateTime(m.created_at)}</td>
              <td className="px-2 py-2">
                {m.party_id ? (
                  <Link className="text-sky-700 hover:underline" to={`/clients/${m.party_id}`}>
                    {m.party_name}
                  </Link>
                ) : (
                  '—'
                )}
                <div className="text-xs text-slate-500">{recipientText(m)}</div>
              </td>
              <td className="px-2 py-2">
                {KIND_LABELS[m.kind]}
                {m.sale_number !== null && <div className="text-xs text-slate-500">чек № {m.sale_number}</div>}
                {m.test && <Badge tone="sky">пробное</Badge>}
              </td>
              <td className="px-2 py-2">
                {CHANNEL_LABELS[m.channel]}
                {m.template && <div className="text-xs text-slate-500">шаблон {m.template}</div>}
              </td>
              <td className="max-w-md px-2 py-2 text-xs whitespace-pre-wrap">{m.text}</td>
              <td className="px-2 py-2">
                <Badge tone={statusTone(m.status)}>{STATUS_LABELS[m.status]}</Badge>
                {m.sent_at && <div className="text-xs text-slate-500">{formatDateTime(m.sent_at)}</div>}
                {m.error && <div className="text-xs text-rose-700">{m.error}</div>}
              </td>
            </tr>
          ))}
        </Table>
      )}
      <Pager page={list.page} size={list.size} total={list.total} onPage={list.setPage} onSize={list.setSize} label="Страницы сообщений" />
    </Card>
  )
}

/** Журнал сообщений: телефон получателя — текстом, пробное — отдельным столбцом. */
const MESSAGE_COLS: ExportCol<ClientMessage>[] = [
  { label: 'Когда', value: (m) => xl.dateTime(m.created_at) },
  { label: 'Кому', value: (m) => m.party_name ?? '' },
  { label: 'Получатель', value: (m) => recipientText(m) },
  { label: 'Вид', value: (m) => KIND_LABELS[m.kind] },
  { label: 'Чек', value: (m) => (m.sale_number !== null ? xl.int(m.sale_number) : null) },
  { label: 'Канал', value: (m) => CHANNEL_LABELS[m.channel] },
  { label: 'Шаблон', value: (m) => m.template ?? '' },
  { label: 'Текст', value: (m) => m.text },
  { label: 'Итог', value: (m) => STATUS_LABELS[m.status] },
  { label: 'Отправлено', value: (m) => xl.dateTime(m.sent_at) },
  { label: 'Ошибка', value: (m) => m.error ?? '' },
  { label: 'Пробное', value: (m) => (m.test ? 'да' : '') },
]

// «Настройки» → «Сообщения клиентам» (SPEC-32): WhatsApp Business Platform — шаблоны Meta и их
// переменные по порядку, пробег в день по умолчанию, «Отправить пробное» и журнал сообщений.
// Только владелец.
import { useState } from 'react'
import { Link } from 'react-router-dom'
import { Pager, usePagedLoad } from '../components/Pager'
import { Badge, Button, Card, CardTitle, Checkbox, Empty, ErrorBox, Field, Loading, Notice, PageHeader, Table, toast } from '../components/ui'
import { get, post, put, qs } from '../lib/api'
import {
  CHANNEL_LABELS,
  KIND_LABELS,
  OIL_TEMPLATE_TEXT,
  PURCHASE_TEMPLATE_TEXT,
  STATUS_LABELS,
  languageProblem,
  paramLines,
  recipientText,
  statusTone,
  templateNameProblem,
  type ClientMessage,
  type MessageKind,
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
        subtitle="Напоминание о замене масла и сообщение юрлицу о закупе: в Телеграм, если клиент подключён к боту, иначе — в WhatsApp"
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

function SettingsForm({ s }: { s: MessageSettingsOut }) {
  const [wa, setWa] = useState(s.whatsapp_enabled)
  const [oil, setOil] = useState(s.oil_template)
  const [oilLang, setOilLang] = useState(s.oil_language)
  const [buy, setBuy] = useState(s.purchase_template)
  const [buyLang, setBuyLang] = useState(s.purchase_language)
  const [km, setKm] = useState(String(s.km_per_day))
  const save = useAction()
  const kmValue = Number(km)
  const kmOk = Number.isInteger(kmValue) && kmValue >= 1 && kmValue <= 1000
  const bad =
    !kmOk ||
    templateNameProblem(oil) !== null ||
    templateNameProblem(buy) !== null ||
    languageProblem(oilLang) !== null ||
    languageProblem(buyLang) !== null
  return (
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
      <Checkbox label="Отправлять в WhatsApp клиентам, не подключённым к боту" checked={wa} onChange={setWa} />
      <div className="grid gap-3 lg:grid-cols-2">
        <TemplateBox
          id="oil"
          title="Замена масла"
          text={OIL_TEMPLATE_TEXT}
          params={s.oil_params}
          name={oil}
          lang={oilLang}
          onName={setOil}
          onLang={setOilLang}
        />
        <TemplateBox
          id="purchase"
          title="Закуп юрлица"
          text={PURCHASE_TEMPLATE_TEXT}
          params={s.purchase_params}
          name={buy}
          lang={buyLang}
          onName={setBuy}
          onLang={setBuyLang}
        />
      </div>
      <Field
        label="Пробег в день по умолчанию, км"
        hint="Если в книжке машины одна запись с пробегом; иначе — средний по книжке"
        error={kmOk ? null : 'от 1 до 1000'}
      >
        <input id="km-per-day" className="w-28" inputMode="numeric" value={km} onChange={(e) => setKm(e.target.value)} />
      </Field>
      <ErrorBox error={save.error} />
      <div>
        <Button
          disabled={bad || save.busy}
          onClick={() =>
            void save.run(async () => {
              await put('/settings/client-messages', {
                whatsapp_enabled: wa,
                oil_template: oil.trim(),
                oil_language: oilLang.trim(),
                purchase_template: buy.trim(),
                purchase_language: buyLang.trim(),
                km_per_day: kmValue,
              })
              toast('Сохранено')
            })
          }
        >
          Сохранить
        </Button>
      </div>
    </Card>
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
      <CardTitle actions={<Button variant="secondary" className="min-h-[34px] px-3 py-1 text-xs" onClick={list.reload}>Обновить</Button>}>
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

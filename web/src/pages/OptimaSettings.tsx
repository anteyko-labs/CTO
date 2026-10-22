// «Настройки» → «Optima Business» (SPEC-31): ID компании, расчётный счёт, торговая точка и касса
// для QR на сумму чека, безналичный счёт учёта для этих денег, «Проверить связь». Только владелец.
import { useState } from 'react'
import { Link } from 'react-router-dom'
import { Button, Card, CardTitle, ErrorBox, Field, Loading, Notice, PageHeader, toast } from '../components/ui'
import { get, post, put, qs } from '../lib/api'
import { useAction, useLoad } from '../lib/hooks'
import { accountNumberProblem, type OptimaSettings as Settings, type SalePoint } from '../lib/optima'
import type { CashAccount } from '../lib/types'

export default function OptimaSettings() {
  const st = useLoad(() => get<Settings>('/settings/optima'), [])
  if (st.loading && !st.data) return <Loading />
  if (!st.data) return <ErrorBox error={st.error} />
  // Форма начинается с сохранённой настройки; после «Сохранить» в ней то, что сохранено.
  return <OptimaForm s={st.data} />
}

function OptimaForm({ s }: { s: Settings }) {
  const banks = useLoad(() => get<CashAccount[]>('/cash/accounts').then((all) => all.filter((a) => a.kind === 'bank' && a.active)), [])
  const [legal, setLegal] = useState(s.legal_party_id ? String(s.legal_party_id) : '')
  const [account, setAccount] = useState(s.account_number)
  const [point, setPoint] = useState<{ code: number; name: string } | null>(s.sale_point_code !== null ? { code: s.sale_point_code, name: s.sale_point_name } : null)
  const [cash, setCash] = useState<{ code: number; name: string } | null>(s.cash_code !== null ? { code: s.cash_code, name: s.cash_name } : null)
  const [moneyAccount, setMoneyAccount] = useState(s.cash_account_id ?? '')
  const [points, setPoints] = useState<SalePoint[] | null>(null)
  const [checkText, setCheckText] = useState<{ ok: boolean; message: string } | null>(null)
  const load = useAction()
  const save = useAction()
  const check = useAction()

  const legalId = /^\d{1,12}$/.test(legal.trim()) ? Number(legal.trim()) : null
  const accountProblem = accountNumberProblem(account)
  const shownPoint = points?.find((p) => p.code === point?.code)

  return (
    <div className="flex max-w-3xl flex-col gap-4">
      <PageHeader title="Optima Business" subtitle="QR на сумму чека и сверка с выпиской банка с настоящей комиссией" />
      {!s.enabled && (
        <Notice>
          Ключ банка не задан на сервере: QR на сумму и сверка выключены, касса работает как раньше. Ключ создаёт первый подписант в Optima Business
          (категории: генерация QR, статус QR-платежа, список точек и касс, выписка по POS и QR); на точке его вписывают в <code>config.ps1</code>{' '}
          (<code>$OptimaApiKey</code>) и перезапускают сервер.
        </Notice>
      )}
      <Card className="flex flex-col gap-3">
        <CardTitle>Компания и счёт</CardTitle>
        <div className="grid gap-3 sm:grid-cols-2">
          <Field label="ID компании в банке" hint="legalPartyId из Optima Business">
            <input id="optima-legal" inputMode="numeric" value={legal} onChange={(e) => setLegal(e.target.value)} />
          </Field>
          <Field label="Расчётный счёт, 16 цифр" hint="Для выписки по POS и QR" error={accountProblem}>
            <input id="optima-account" inputMode="numeric" value={account} onChange={(e) => setAccount(e.target.value)} />
          </Field>
        </div>
        <Field label="Счёт учёта для денег банка" hint="Куда ложатся оплаты по QR и поправки комиссии; пусто — счёт по умолчанию для карты">
          <select id="optima-money-account" value={moneyAccount} onChange={(e) => setMoneyAccount(e.target.value)}>
            <option value="">Счёт по умолчанию для карты</option>
            {(banks.data ?? []).map((a) => (
              <option key={a.id} value={a.id}>
                {a.name}
              </option>
            ))}
          </select>
        </Field>
      </Card>
      <Card className="flex flex-col gap-3">
        <CardTitle
          actions={
            <Button
              variant="secondary"
              disabled={!s.enabled || legalId === null || load.busy}
              onClick={() =>
                void load.run(async () => {
                  const list = await get<SalePoint[]>(`/settings/optima/sale-points${qs({ legal_party_id: legalId })}`)
                  setPoints(list)
                  if (list.length === 0) toast('У компании в банке нет торговых точек', 'error')
                })
              }
            >
              {load.busy ? 'Загружаем…' : 'Загрузить точки и кассы'}
            </Button>
          }
        >
          Торговая точка и касса для QR
        </CardTitle>
        <ErrorBox error={load.error} />
        <div className="text-sm text-slate-600">
          Сейчас: {point ? `точка «${point.name || point.code}»` : 'точка не выбрана'}, {cash ? `касса «${cash.name || cash.code}»` : 'касса не выбрана'}
        </div>
        {points && points.length > 0 && (
          <div className="grid gap-3 sm:grid-cols-2">
            <Field label="Торговая точка">
              <select
                id="optima-point"
                value={point ? String(point.code) : ''}
                onChange={(e) => {
                  const p = points.find((x) => String(x.code) === e.target.value)
                  setPoint(p ? { code: p.code, name: p.name } : null)
                  setCash(p && p.cashes.length === 1 ? { code: p.cashes[0].code, name: p.cashes[0].name } : null)
                }}
              >
                <option value="">— выберите —</option>
                {points.map((p) => (
                  <option key={p.code} value={p.code}>
                    {p.name || p.code}
                    {p.address ? ` — ${p.address}` : ''}
                  </option>
                ))}
              </select>
            </Field>
            <Field label="Касса">
              <select
                id="optima-cash"
                value={cash ? String(cash.code) : ''}
                disabled={!shownPoint}
                onChange={(e) => {
                  const c = shownPoint?.cashes.find((x) => String(x.code) === e.target.value)
                  setCash(c ? { code: c.code, name: c.name } : null)
                }}
              >
                <option value="">— выберите —</option>
                {(shownPoint?.cashes ?? []).map((c) => (
                  <option key={c.code} value={c.code}>
                    {c.name || c.code}
                  </option>
                ))}
              </select>
            </Field>
          </div>
        )}
      </Card>
      <ErrorBox error={save.error} />
      {checkText && <Notice tone={checkText.ok ? 'green' : 'rose'}>{checkText.message}</Notice>}
      <ErrorBox error={check.error} />
      <div className="flex flex-wrap gap-2">
        <Button
          disabled={save.busy || accountProblem !== null || (legal.trim() !== '' && legalId === null)}
          onClick={() =>
            void save.run(async () => {
              await put<Settings>('/settings/optima', {
                legal_party_id: legalId,
                account_number: account.replace(/\s/g, ''),
                sale_point_code: point?.code ?? null,
                sale_point_name: point?.name ?? '',
                cash_code: cash?.code ?? null,
                cash_name: cash?.name ?? '',
                cash_account_id: moneyAccount || null,
              })
              toast('Настройка сохранена')
            })
          }
        >
          Сохранить
        </Button>
        <Button
          variant="secondary"
          disabled={!s.enabled || check.busy || save.busy}
          onClick={() =>
            void check.run(async () => {
              setCheckText(await post<{ ok: boolean; message: string }>('/settings/optima/check'))
            })
          }
        >
          {check.busy ? 'Проверяем…' : 'Проверить связь'}
        </Button>
        <Link to="/bank/reconcile" className="inline-flex min-h-[42px] items-center px-2 text-sm text-sky-700 hover:underline">
          Сверка с банком →
        </Link>
      </div>
    </div>
  )
}

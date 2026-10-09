// Документы о долге и акт сверки (SPEC-10): тексты по умолчанию, подстановки, сумма прописью, печать A4.
// Тексты правит владелец в настройках; пустое значение подстановки печатается линией для записи от руки.
import { formatDateTime, formatKg, formatLiters, formatSom } from './format'
import type { Sale } from './types'

export interface Seller {
  name: string
  inn: string
  address: string
  phone: string
  director: string
  bank: string
  city: string
}

export interface DebtDocSettings {
  seller: Seller
  person: string | null
  company: string | null
}

export interface ReconRow {
  date: string
  document: string
  doc_type: string
  doc_id: string | null
  debit_tyiyn: number
  credit_tyiyn: number
  comment: string
}

export interface Reconciliation {
  party_id: string
  name: string
  kind: 'person' | 'company'
  role: 'customer' | 'supplier'
  inn: string
  phone: string
  from: string
  to: string
  opening_tyiyn: number
  rows: ReconRow[]
  debit_total_tyiyn: number
  credit_total_tyiyn: number
  closing_tyiyn: number
}

export const BLANK = '________________'

// ---------- Сумма прописью ----------

const ONES_M = ['', 'один', 'два', 'три', 'четыре', 'пять', 'шесть', 'семь', 'восемь', 'девять']
const ONES_F = ['', 'одна', 'две', 'три', 'четыре', 'пять', 'шесть', 'семь', 'восемь', 'девять']
const TEENS = ['десять', 'одиннадцать', 'двенадцать', 'тринадцать', 'четырнадцать', 'пятнадцать', 'шестнадцать', 'семнадцать', 'восемнадцать', 'девятнадцать']
const TENS = ['', '', 'двадцать', 'тридцать', 'сорок', 'пятьдесят', 'шестьдесят', 'семьдесят', 'восемьдесят', 'девяносто']
const HUNDREDS = ['', 'сто', 'двести', 'триста', 'четыреста', 'пятьсот', 'шестьсот', 'семьсот', 'восемьсот', 'девятьсот']

/** Форма слова по числу: 1 тысяча, 2 тысячи, 5 тысяч. */
function plural(n: number, forms: [string, string, string]): string {
  const n100 = n % 100
  const n10 = n % 10
  if (n100 >= 11 && n100 <= 19) return forms[2]
  if (n10 === 1) return forms[0]
  if (n10 >= 2 && n10 <= 4) return forms[1]
  return forms[2]
}

function triad(n: number, feminine: boolean): string[] {
  const words: string[] = []
  const h = Math.trunc(n / 100)
  const rest = n % 100
  if (h) words.push(HUNDREDS[h])
  if (rest >= 10 && rest <= 19) words.push(TEENS[rest - 10])
  else {
    const t = Math.trunc(rest / 10)
    const o = rest % 10
    if (t) words.push(TENS[t])
    if (o) words.push((feminine ? ONES_F : ONES_M)[o])
  }
  return words
}

/** Целое число прописью, мужской род: «одна тысяча двести один». */
export function numberInWords(n: number): string {
  if (!Number.isSafeInteger(n) || n < 0) return String(n)
  if (n === 0) return 'ноль'
  const scales: { forms: [string, string, string]; feminine: boolean }[] = [
    { forms: ['', '', ''], feminine: false },
    { forms: ['тысяча', 'тысячи', 'тысяч'], feminine: true },
    { forms: ['миллион', 'миллиона', 'миллионов'], feminine: false },
    { forms: ['миллиард', 'миллиарда', 'миллиардов'], feminine: false },
  ]
  const parts: string[] = []
  let rest = n
  for (let i = 0; rest > 0 && i < scales.length; i++) {
    const t = rest % 1000
    rest = Math.trunc(rest / 1000)
    if (!t) continue
    const words = triad(t, scales[i].feminine)
    if (i > 0) words.push(plural(t, scales[i].forms))
    parts.unshift(words.join(' '))
  }
  return parts.join(' ')
}

/** Сомы прописью, тыйыны цифрами: «одна тысяча двести сом 50 тыйын». */
export function somInWords(tyiyn: number): string {
  const abs = Math.abs(tyiyn)
  const som = Math.trunc(abs / 100)
  const t = abs % 100
  const words = `${tyiyn < 0 ? 'минус ' : ''}${numberInWords(som)} сом ${String(t).padStart(2, '0')} тыйын`
  return words.charAt(0).toUpperCase() + words.slice(1)
}

// ---------- Подстановки ----------

const esc = (s: string) =>
  s.replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c] ?? c)

/**
 * Подставляет значения в шаблон. Текст экранируется, переносы строк сохраняются;
 * значения из `html` вставляются как есть (таблица товаров). Нет данных — линия.
 */
export function renderTemplate(template: string, vars: Record<string, string>, html: Record<string, string> = {}): string {
  return esc(template).replace(/\{\{\s*([^}\s]+)\s*\}\}/g, (_, key: string) => {
    if (key in html) return html[key]
    const v = vars[key]
    return esc(v && v.trim() ? v : BLANK)
  })
}

/** Список подстановок для подсказки в настройках. */
export const PLACEHOLDERS: [string, string][] = [
  ['дата', 'дата чека'],
  ['город', 'город точки'],
  ['продавец', 'название точки (ИП / ОсОО)'],
  ['инн_продавца', 'ИНН точки'],
  ['адрес_продавца', 'адрес точки'],
  ['телефон_продавца', 'телефон точки'],
  ['руководитель', 'кто подписывает от точки'],
  ['клиент', 'ФИО или название покупателя'],
  ['инн', 'ПИН физлица или ИНН фирмы'],
  ['телефон', 'телефон покупателя'],
  ['работник', 'кто приехал от фирмы'],
  ['машина', 'госномер'],
  ['чек', 'номер чека'],
  ['товары', 'таблица: наименование, количество, цена, сумма'],
  ['сумма', 'сумма в долг'],
  ['сумма_словами', 'сумма прописью'],
  ['срок_оплаты', 'дата, до которой платить'],
  ['история_долга', 'таблица прежних долгов и оплат с момента, когда долг был закрыт'],
  ['долг_до', 'сколько был должен до этой покупки'],
  ['баланс', 'весь долг с учётом этого чека'],
  ['баланс_словами', 'весь долг прописью'],
  ['кассир', 'кто отпустил товар'],
]

// ---------- Тексты по умолчанию (SPEC-10) ----------

export const DEFAULT_PERSON = `РАСПИСКА
о получении товара с отсрочкой оплаты (в долг)

г. {{город}}                                                                 {{дата}}

Я, {{клиент}}, ПИН {{инн}},
паспорт: серия ______ № ______________, выдан ________________________________ «___» __________ ____ г.,
зарегистрирован(а) по адресу: ______________________________________________, телефон {{телефон}},

получил(а) от {{продавец}}, ИНН {{инн_продавца}}, адрес: {{адрес_продавца}},
товар по чеку № {{чек}} от {{дата}}:

{{товары}}

на общую сумму {{сумма}} ({{сумма_словами}}).

Ранее полученное в долг и оплаты:
{{история_долга}}

Задолженность до этой покупки: {{долг_до}}. Взято в долг сейчас: {{сумма}}.
Общая сумма моей задолженности перед продавцом с учётом этой покупки составляет {{баланс}} ({{баланс_словами}}).

Обязуюсь оплатить всю задолженность в срок до {{срок_оплаты}} наличными в кассу продавца, картой или по QR.

При нарушении срока оплаты продавец вправе требовать погашения долга, а также уплаты процентов за пользование чужими денежными средствами в порядке, установленном законодательством Кыргызской Республики.

Товар получен, претензий к количеству, качеству и комплектности не имею.
Расписка составлена в двух экземплярах, имеющих одинаковую силу, по одному для каждой стороны, и подписана мной добровольно.

Покупатель:  ________________ / {{клиент}} /

Продавец:    ________________ / {{кассир}} /`

export const DEFAULT_COMPANY = `НАКЛАДНАЯ № {{чек}} от {{дата}}
на отпуск товара с отсрочкой оплаты

Поставщик:  {{продавец}}, ИНН {{инн_продавца}}, адрес: {{адрес_продавца}}, тел. {{телефон_продавца}}
Покупатель: {{клиент}}, ИНН {{инн}}, тел. {{телефон}}
Получатель: {{работник}}, по доверенности № ________ от ______________
Автомобиль: {{машина}}

{{товары}}

Всего отпущено на сумму {{сумма}} ({{сумма_словами}}).

Ранее отпущено с отсрочкой оплаты и оплачено:
{{история_долга}}

Задолженность покупателя до этой накладной: {{долг_до}}. По этой накладной: {{сумма}}.
Задолженность покупателя перед поставщиком с учётом этой накладной: {{баланс}} ({{баланс_словами}}).
Покупатель обязуется оплатить задолженность до {{срок_оплаты}}.
Товар получен в полном объёме, претензий к количеству, качеству и комплектности нет.

Отпустил:  ________________ / {{кассир}} /                    М.П.

Получил:   ________________ / {{работник}} /                  М.П.`

// ---------- Печать ----------

const A4_STYLE = `
  @page { size: A4; margin: 15mm; }
  body { font-family: 'Times New Roman', serif; font-size: 11pt; color: #000; margin: 0; }
  .doc { white-space: pre-wrap; line-height: 1.45; }
  .copy + .copy { page-break-before: always; }
  table { border-collapse: collapse; width: 100%; margin: 2mm 0; white-space: normal; }
  th, td { border: 1px solid #000; padding: 1mm 1.5mm; font-size: 10pt; vertical-align: top; }
  th { background: #f2f2f2; }
  .r { text-align: right; white-space: nowrap; }
  .c { text-align: center; }
  h1 { font-size: 13pt; text-align: center; margin: 0 0 3mm; }
  .sign { display: flex; justify-content: space-between; gap: 10mm; margin-top: 10mm; }
  .sign > div { flex: 1; }
  .muted { color: #444; font-size: 9pt; }
`

export function printHtml(title: string, body: string): void {
  const frame = document.createElement('iframe')
  Object.assign(frame.style, { position: 'fixed', width: '0', height: '0', border: '0' })
  document.body.appendChild(frame)
  const doc = frame.contentDocument
  const win = frame.contentWindow
  if (!doc || !win) {
    frame.remove()
    return
  }
  doc.open()
  doc.write(`<!doctype html><html><head><meta charset="utf-8"><title>${esc(title)}</title><style>${A4_STYLE}</style></head><body>${body}</body></html>`)
  doc.close()
  win.focus()
  win.print()
  setTimeout(() => frame.remove(), 60_000)
}

function qtyText(l: Sale['lines'][number]): string {
  if (l.kind === 'pour') return formatLiters(l.qty)
  if (l.kind === 'container') return `${l.qty} кан.${l.container_ml ? ` × ${formatLiters(l.container_ml)}` : ''}`
  if (l.kind === 'weight') return formatKg(l.qty)
  if (l.kind === 'service') return `${l.qty} усл.`
  return `${l.qty} шт`
}

function itemsTable(sale: Sale): string {
  const rows = sale.lines
    .map(
      (l, i) => `<tr><td class="c">${i + 1}</td><td>${esc(l.name)}${l.gift ? ' (подарок)' : ''}</td>
        <td class="r">${esc(qtyText(l))}</td><td class="r">${esc(formatSom(l.unit_price_tyiyn))}${l.kind === 'pour' ? '/л' : l.kind === 'weight' ? '/кг' : ''}</td>
        <td class="r">${esc(formatSom(l.amount_tyiyn))}</td></tr>`,
    )
    .join('')
  return `<table><thead><tr><th>№</th><th>Наименование</th><th>Кол-во</th><th>Цена</th><th>Сумма</th></tr></thead>
    <tbody>${rows}<tr><td colspan="4" class="r"><b>Итого</b></td><td class="r"><b>${esc(formatSom(sale.total_tyiyn))}</b></td></tr></tbody></table>`
}

const dateRu = (iso: string) => {
  const [y, m, d] = iso.slice(0, 10).split('-')
  return `${d}.${m}.${y}`
}

/** Дата чека плюс срок оплаты в днях — без часового пояса устройства. */
function addDays(iso: string, days: number): string {
  const [y, m, d] = iso.slice(0, 10).split('-').map(Number)
  const t = new Date(Date.UTC(y, m - 1, d + days))
  return t.toISOString().slice(0, 10)
}

export interface DebtDocInput {
  sale: Sale
  party: { name: string; kind: 'person' | 'company'; inn: string; phone: string; due_days: number | null }
  settings: DebtDocSettings
  /** Вся история долга клиента (акт сверки с первой операции). */
  history?: Reconciliation | null
}

export interface DebtHistory {
  /** Записи с момента, когда долг последний раз был закрыт, до этого чека. */
  rows: ReconRow[]
  before: number
  after: number
}

/**
 * Прежний долг к этому чеку: идём по истории, находим чек и берём записи после того,
 * как долг последний раз был закрыт (баланс ≤ 0). Без чека в истории — null.
 */
export function debtHistory(history: Reconciliation, saleId: string): DebtHistory | null {
  let running = history.opening_tyiyn
  let start = 0
  for (let i = 0; i < history.rows.length; i++) {
    const r = history.rows[i]
    if (r.doc_type === 'sale' && r.doc_id === saleId) {
      const before = running
      return { rows: history.rows.slice(start, i), before, after: before + r.debit_tyiyn - r.credit_tyiyn }
    }
    running += r.debit_tyiyn - r.credit_tyiyn
    if (running <= 0) start = i + 1
  }
  return null
}

function historyTable(h: DebtHistory | null): string {
  if (!h || h.rows.length === 0) return '<div>Ранее задолженности не было.</div>'
  const money = (v: number) => (v ? esc(formatSom(v)) : '')
  const rows = h.rows
    .map(
      (r) => `<tr><td>${dateRu(r.date)}</td><td>${esc(r.document)}</td>
        <td class="r">${money(r.debit_tyiyn)}</td><td class="r">${money(r.credit_tyiyn)}</td></tr>`,
    )
    .join('')
  return `<table><thead><tr><th>Дата</th><th>Основание</th><th>Взято в долг</th><th>Оплачено</th></tr></thead>
    <tbody>${rows}<tr><td colspan="2" class="r"><b>Задолженность до этой покупки</b></td>
    <td colspan="2" class="r"><b>${esc(formatSom(h.before))}</b></td></tr></tbody></table>`
}

/** Документ о долге по чеку: расписка физлица в двух экземплярах или накладная юрлица. */
export function printDebtDoc({ sale, party, settings, history }: DebtDocInput): void {
  const debt = sale.payments.filter((p) => p.method === 'debt').reduce((a, p) => a + p.amount_tyiyn, 0)
  // Баланс — на момент этого чека по истории, а не сегодняшний: документ печатают и позже.
  const h = history ? debtHistory(history, sale.id) : null
  const after = h ? h.after : sale.party_balance_tyiyn
  const before = h ? h.before : after === null ? null : after - debt
  const company = party.kind === 'company'
  const template = (company ? settings.company : settings.person) || (company ? DEFAULT_COMPANY : DEFAULT_PERSON)
  const saleDate = sale.created_at.slice(0, 10)
  const s = settings.seller
  const vars: Record<string, string> = {
    дата: dateRu(saleDate),
    город: s.city,
    продавец: s.name,
    инн_продавца: s.inn,
    адрес_продавца: s.address,
    телефон_продавца: s.phone,
    руководитель: s.director,
    клиент: party.name,
    инн: party.inn,
    телефон: party.phone,
    работник: sale.contact_name ?? '',
    машина: sale.vehicle_plate ?? '',
    чек: String(sale.number),
    сумма: formatSom(debt),
    сумма_словами: somInWords(debt),
    срок_оплаты: party.due_days ? dateRu(addDays(saleDate, party.due_days)) : '',
    баланс: after === null ? '' : formatSom(after),
    баланс_словами: after === null ? '' : somInWords(after),
    долг_до: before === null ? '' : formatSom(Math.max(before, 0)),
    кассир: sale.cashier_name,
  }
  const body = `<div class="doc">${renderTemplate(template, vars, { товары: itemsTable(sale), история_долга: historyTable(h) })}</div>`
  const copies = company ? 1 : 2
  printHtml(
    `${company ? 'Накладная' : 'Расписка'} № ${sale.number}`,
    Array.from({ length: copies }, () => `<div class="copy">${body}</div>`).join(''),
  )
}

/** Акт сверки взаимных расчётов: данные точки полностью, графы контрагента — для его отметок. */
export function printReconciliation(act: Reconciliation, seller: Seller): void {
  const us = seller.name || BLANK
  const them = act.name
  const money = (v: number) => (v ? esc(formatSom(v)) : '')
  const rows = act.rows
    .map(
      (r) => `<tr><td>${dateRu(r.date)}</td><td>${esc(r.document)}${r.comment ? `<div class="muted">${esc(r.comment)}</div>` : ''}</td>
        <td class="r">${money(r.debit_tyiyn)}</td><td class="r">${money(r.credit_tyiyn)}</td>
        <td></td><td></td><td></td><td></td></tr>`,
    )
    .join('')
  const opening = act.opening_tyiyn
  const closing = act.closing_tyiyn
  const saldoRow = (label: string, v: number) =>
    `<tr><td colspan="2"><b>${label}</b></td><td class="r"><b>${v > 0 ? esc(formatSom(v)) : ''}</b></td>
      <td class="r"><b>${v < 0 ? esc(formatSom(-v)) : ''}</b></td><td colspan="2"><b>${label}</b></td><td></td><td></td></tr>`
  const verdict =
    closing === 0
      ? `Задолженность между сторонами на ${dateRu(act.to)} отсутствует.`
      : closing > 0
        ? `На ${dateRu(act.to)} задолженность ${esc(them)} перед ${esc(us)} составляет ${esc(formatSom(closing))} (${esc(somInWords(closing))}).`
        : `На ${dateRu(act.to)} задолженность ${esc(us)} перед ${esc(them)} составляет ${esc(formatSom(-closing))} (${esc(somInWords(-closing))}).`
  const body = `
    <h1>АКТ СВЕРКИ ВЗАИМНЫХ РАСЧЁТОВ<br>за период с ${dateRu(act.from)} по ${dateRu(act.to)}</h1>
    <p>между ${esc(us)}${seller.inn ? `, ИНН ${esc(seller.inn)}` : ''} и ${esc(them)}${act.inn ? `, ИНН ${esc(act.inn)}` : ''}</p>
    <p>Мы, нижеподписавшиеся, ${esc(seller.director || BLANK)} от ${esc(us)}, с одной стороны, и ${BLANK} от ${esc(them)}, с другой стороны,
      составили настоящий акт о том, что состояние взаимных расчётов по данным учёта следующее:</p>
    <table>
      <thead>
        <tr><th colspan="4">По данным ${esc(us)}, сом</th><th colspan="4">По данным ${esc(them)}, сом</th></tr>
        <tr><th>Дата</th><th>Документ</th><th>Дебет</th><th>Кредит</th><th>Дата</th><th>Документ</th><th>Дебет</th><th>Кредит</th></tr>
      </thead>
      <tbody>
        ${saldoRow(`Сальдо на ${dateRu(act.from)}`, opening)}
        ${rows}
        <tr><td colspan="2"><b>Обороты за период</b></td><td class="r"><b>${esc(formatSom(act.debit_total_tyiyn))}</b></td>
          <td class="r"><b>${esc(formatSom(act.credit_total_tyiyn))}</b></td><td colspan="2"><b>Обороты за период</b></td><td></td><td></td></tr>
        ${saldoRow(`Сальдо на ${dateRu(act.to)}`, closing)}
      </tbody>
    </table>
    <p>По данным ${esc(us)}: ${verdict}</p>
    <p>По данным ${esc(them)}: ______________________________________________________________</p>
    <div class="sign">
      <div>От ${esc(us)}<br><br>________________ / ${esc(seller.director || '________________')} /<br><br>М.П.</div>
      <div>От ${esc(them)}<br><br>________________ / ________________ /<br><br>М.П.</div>
    </div>
    <p class="muted">Сформировано ${esc(formatDateTime(new Date().toISOString()))}. Дебет — в пользу ${esc(us)} (отгрузка покупателю, оплата поставщику), кредит — в пользу ${esc(them)}.</p>`
  printHtml(`Акт сверки — ${them}`, body)
}

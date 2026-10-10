// Реквизиты точки и тексты документов о долге (SPEC-10). Правит владелец.
import { useEffect, useRef, useState } from 'react'
import { Button, Card, CardTitle, ErrorBox, Field, Loading, PageHeader, toast } from '../components/ui'
import { get, put } from '../lib/api'
import { DEFAULT_COMPANY, DEFAULT_PERSON, PLACEHOLDERS, renderTemplate, somInWords, type DebtDocSettings, type Seller } from '../lib/debtDocs'
import { formatSomExact, todayBishkek } from '../lib/format'
import { useAction, useLoad } from '../lib/hooks'

const SELLER_FIELDS: [keyof Seller, string, string][] = [
  ['name', 'Название точки', 'ИП Асанов А. А. или ОсОО «Автодом»'],
  ['inn', 'ИНН', '14 цифр'],
  ['address', 'Адрес', 'г. Бишкек, ул. …'],
  ['city', 'Город для документов', 'Бишкек'],
  ['phone', 'Телефон', '+996 …'],
  ['director', 'Кто подписывает от точки', 'Асанов А. А.'],
  ['bank', 'Банк и счёт', 'для акта сверки и оплаты переводом'],
]

// Образец для предпросмотра: подставляются реквизиты точки из формы и придуманный покупатель.
const SAMPLE_DEBT = 350_000
const SAMPLE_BEFORE = 120_000
const SAMPLE_ITEMS = `<table><thead><tr><th>№</th><th>Наименование</th><th>Кол-во</th><th>Цена</th><th>Сумма</th></tr></thead>
<tbody><tr><td class="c">1</td><td>Масло моторное 5W-30, канистра 4 л</td><td class="r">1 шт</td><td class="r">${formatSomExact(300_000)}</td><td class="r">${formatSomExact(300_000)}</td></tr>
<tr><td class="c">2</td><td>Фильтр масляный</td><td class="r">1 шт</td><td class="r">${formatSomExact(50_000)}</td><td class="r">${formatSomExact(50_000)}</td></tr>
<tr><td colspan="4" class="r"><b>Итого</b></td><td class="r"><b>${formatSomExact(SAMPLE_DEBT)}</b></td></tr></tbody></table>`
const SAMPLE_HISTORY = `<table><thead><tr><th>Дата</th><th>Основание</th><th>Взято в долг</th><th>Оплачено</th></tr></thead>
<tbody><tr><td>01.10.2026</td><td>Отгрузка товара, чек № 101</td><td class="r">${formatSomExact(200_000)}</td><td></td></tr>
<tr><td>05.10.2026</td><td>Оплата наличными</td><td></td><td class="r">${formatSomExact(80_000)}</td></tr>
<tr><td colspan="2" class="r"><b>Задолженность до этой покупки</b></td><td colspan="2" class="r"><b>${formatSomExact(SAMPLE_BEFORE)}</b></td></tr></tbody></table>`

function sampleVars(seller: Seller, company: boolean): Record<string, string> {
  const [y, m, d] = todayBishkek().split('-')
  const after = SAMPLE_BEFORE + SAMPLE_DEBT
  return {
    дата: `${d}.${m}.${y}`,
    город: seller.city,
    продавец: seller.name,
    инн_продавца: seller.inn,
    адрес_продавца: seller.address,
    телефон_продавца: seller.phone,
    руководитель: seller.director,
    клиент: company ? 'ОсОО «Пример Транс»' : 'Иванов Иван Иванович',
    инн: company ? '01234567890123' : '21234567890123',
    телефон: '+996 555 123 456',
    работник: company ? 'Водитель Петров П.' : '',
    машина: '01KG123ABC',
    чек: '125',
    сумма: formatSomExact(SAMPLE_DEBT),
    сумма_словами: somInWords(SAMPLE_DEBT),
    срок_оплаты: `${d}.${m}.${y}`,
    баланс: formatSomExact(after),
    баланс_словами: somInWords(after),
    долг_до: formatSomExact(SAMPLE_BEFORE),
    кассир: 'Кассир Асель',
  }
}

/** Текст шаблона с кнопками подстановок и вкладкой предпросмотра на образце. */
function TemplateCard({
  title,
  hint,
  value,
  fallback,
  seller,
  company,
  onChange,
}: {
  title: string
  hint: string
  value: string
  fallback: string
  seller: Seller
  company: boolean
  onChange: (v: string) => void
}) {
  const [tab, setTab] = useState<'text' | 'preview'>('text')
  const area = useRef<HTMLTextAreaElement>(null)

  /** Вставляет подстановку туда, где стоит курсор, и ставит курсор после неё. */
  const insert = (key: string) => {
    const el = area.current
    const token = `{{${key}}}`
    const start = el ? el.selectionStart : value.length
    const end = el ? el.selectionEnd : value.length
    onChange(value.slice(0, start) + token + value.slice(end))
    requestAnimationFrame(() => {
      if (!el) return
      el.focus()
      el.setSelectionRange(start + token.length, start + token.length)
    })
  }

  return (
    <Card className="flex flex-col gap-3">
      <CardTitle
        actions={
          <Button variant="secondary" onClick={() => onChange(fallback)}>
            Вернуть исходный текст
          </Button>
        }
      >
        {title}
        <span className="block text-xs font-normal text-slate-500">{hint}</span>
      </CardTitle>
      <div role="tablist" className="inline-grid w-fit grid-cols-2 overflow-hidden rounded-md border border-slate-300 text-sm">
        {(
          [
            ['text', 'Текст'],
            ['preview', 'Предпросмотр'],
          ] as const
        ).map(([v, label]) => (
          <button
            key={v}
            type="button"
            role="tab"
            aria-selected={tab === v}
            className={`min-h-[38px] px-4 py-1.5 ${tab === v ? 'bg-sky-600 text-white' : 'bg-white hover:bg-slate-50'}`}
            onClick={() => setTab(v)}
          >
            {label}
          </button>
        ))}
      </div>
      {tab === 'text' ? (
        <>
          <div className="flex flex-wrap gap-1.5" aria-label="Подстановки">
            {PLACEHOLDERS.map(([key, h]) => (
              <button
                key={key}
                type="button"
                title={h}
                className="rounded-full border border-slate-300 bg-slate-50 px-2.5 py-1 font-mono text-xs text-slate-700 hover:border-sky-400 hover:bg-sky-50"
                onClick={() => insert(key)}
              >
                {key}
              </button>
            ))}
          </div>
          <div className="text-xs text-slate-500">Нажмите подстановку — она встанет туда, где курсор. Наведите, чтобы увидеть, что подставится.</div>
          <textarea ref={area} className="min-h-[360px] font-mono text-xs" value={value} onChange={(e) => onChange(e.target.value)} />
        </>
      ) : (
        <>
          <div className="text-xs text-slate-500">Образец с придуманным покупателем и вашими реквизитами. Пустые подстановки печатаются линией.</div>
          <div
            className="overflow-x-auto rounded-md border border-slate-300 p-6 font-serif text-sm leading-relaxed whitespace-pre-wrap shadow-inner [&_.c]:text-center [&_.r]:text-right [&_.r]:whitespace-nowrap [&_table]:my-1 [&_table]:w-full [&_table]:border-collapse [&_table]:whitespace-normal [&_td]:border [&_td]:border-black [&_td]:px-1.5 [&_td]:py-0.5 [&_th]:border [&_th]:border-black [&_th]:px-1.5 [&_th]:py-0.5"
            style={{ background: '#fff', color: '#000' }}
            // Текст шаблона экранируется в renderTemplate; таблицы образца — наши постоянные строки.
            dangerouslySetInnerHTML={{
              __html: renderTemplate(value || fallback, sampleVars(seller, company), { товары: SAMPLE_ITEMS, история_долга: SAMPLE_HISTORY }),
            }}
          />
        </>
      )}
    </Card>
  )
}

export default function DebtDocsSettings() {
  const loaded = useLoad(() => get<DebtDocSettings>('/settings/debt-docs'), [])
  const [form, setForm] = useState<DebtDocSettings | null>(null)
  const { busy, error, run } = useAction()

  useEffect(() => {
    if (loaded.data && !form) {
      // eslint-disable-next-line react-hooks/set-state-in-effect
      setForm({
        seller: loaded.data.seller,
        person: loaded.data.person || DEFAULT_PERSON,
        company: loaded.data.company || DEFAULT_COMPANY,
      })
    }
  }, [loaded.data, form])

  const save = () =>
    void run(async () => {
      if (!form) return
      // Текст, совпадающий с исходным, не храним: так новая редакция по умолчанию придёт сама.
      await put('/settings/debt-docs', {
        seller: form.seller,
        person: form.person === DEFAULT_PERSON ? null : form.person,
        company: form.company === DEFAULT_COMPANY ? null : form.company,
      })
      toast('Сохранено')
    })

  if (!form) return loaded.error ? <ErrorBox error={loaded.error} /> : <Loading />

  return (
    <div className="flex flex-col gap-4">
      <PageHeader title="Документы о долге" />
      <Card className="flex flex-col gap-3">
        <h2 className="text-base font-semibold">Реквизиты точки</h2>
        <div className="text-xs text-slate-500">Печатаются в расписке, накладной и акте сверки. Пустое поле печатается линией, чтобы вписать от руки.</div>
        <div className="grid gap-3 sm:grid-cols-2">
          {SELLER_FIELDS.map(([key, label, placeholder]) => (
            <Field key={key} label={label}>
              <input
                value={form.seller[key]}
                placeholder={placeholder}
                onChange={(e) => setForm({ ...form, seller: { ...form.seller, [key]: e.target.value } })}
              />
            </Field>
          ))}
        </div>
      </Card>

      <TemplateCard
        title="Физическое лицо: расписка"
        hint="Печатается в двух экземплярах — покупателю и точке"
        value={form.person ?? ''}
        fallback={DEFAULT_PERSON}
        seller={form.seller}
        company={false}
        onChange={(v) => setForm({ ...form, person: v })}
      />
      <TemplateCard
        title="Юридическое лицо: накладная с отсрочкой оплаты"
        hint="Один экземпляр с подписями и печатями обеих сторон"
        value={form.company ?? ''}
        fallback={DEFAULT_COMPANY}
        seller={form.seller}
        company
        onChange={(v) => setForm({ ...form, company: v })}
      />

      <Card>
        <CardTitle>Подстановки</CardTitle>
        <div className="grid gap-x-6 gap-y-1 text-sm sm:grid-cols-2">
          {PLACEHOLDERS.map(([key, hint]) => (
            <div key={key}>
              <code className="rounded bg-slate-100 px-1">{`{{${key}}}`}</code> — {hint}
            </div>
          ))}
        </div>
        <p className="mt-3 text-xs text-slate-500">
          Тексты по умолчанию составлены по общим правилам Гражданского кодекса Кыргызской Республики для продажи с отсрочкой оплаты. Перед использованием
          покажите их своему юристу: условия о сроке, неустойке и порядке взыскания он подстроит под вашу практику.
        </p>
      </Card>

      {/* Кнопка всегда под рукой: страница длинная, а правят обычно один шаблон. */}
      <div className="sticky bottom-[calc(56px+env(safe-area-inset-bottom))] z-20 -mx-1 flex flex-wrap items-center justify-end gap-3 rounded-lg border border-slate-200 bg-white/95 px-3 py-2 shadow-md backdrop-blur md:bottom-2">
        <div className="mr-auto min-w-0">
          <ErrorBox error={error} />
        </div>
        <Button disabled={busy} onClick={save}>
          Сохранить
        </Button>
      </div>
    </div>
  )
}

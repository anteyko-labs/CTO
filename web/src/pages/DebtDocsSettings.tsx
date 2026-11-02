// Реквизиты точки и тексты документов о долге (SPEC-10). Правит владелец.
import { useEffect, useRef, useState } from 'react'
import { Button, Card, CardTitle, ErrorBox, Field, Loading, PageHeader, toast } from '../components/ui'
import { del, get, put } from '../lib/api'
import { SignaturePad } from '../components/SignaturePad'
import { DEFAULT_EXTRA, EXTRA_PLACEHOLDERS, extraDocHtml, printExtraDoc, type ExtraDoc } from '../lib/extraDocs'
import { ACT_PERIODS, DEFAULT_COMPANY, DEFAULT_PERSON, PLACEHOLDERS, pngDataUrl, renderTemplate, somInWords, type ActPeriod, type DebtDocSettings, type Seller } from '../lib/debtDocs'
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
    итого: formatSomExact(SAMPLE_DEBT),
    итого_словами: somInWords(SAMPLE_DEBT),
    из_них: '',
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

/** Подпись руководителя (задача 008): нарисовать мышью или пером, загрузить PNG или убрать.
 *  Задана — встаёт в расписку, накладную и акт сверки сама; нет — линия для ручки. */
function OwnerSignatureCard({ has, onChanged }: { has: boolean; onChanged: () => void }) {
  const [drawing, setDrawing] = useState(false)
  const [version, setVersion] = useState(0)
  const { busy, error, run } = useAction()
  const save = (png: string) =>
    void run(async () => {
      await put('/settings/debt-docs/signature', { signature_png: png })
      setDrawing(false)
      setVersion((v) => v + 1)
      onChanged()
      toast('Подпись сохранена')
    })
  const upload = (file: File | undefined) => {
    if (!file) return
    const r = new FileReader()
    r.onload = () => typeof r.result === 'string' && save(r.result)
    r.readAsDataURL(file)
  }
  const remove = () =>
    void run(async () => {
      await del('/settings/debt-docs/signature')
      onChanged()
      toast('Подпись убрана — в документах линия для ручки')
    })
  return (
    <Card className="flex flex-col gap-3" data-testid="owner-signature">
      <h2 className="text-base font-semibold">Подпись руководителя</h2>
      <div className="text-xs text-slate-500">
        Задана — ставится во все документы о долге и акт сверки сама. Не задана — в документе линия, кассир расписывается ручкой.
      </div>
      <ErrorBox error={error} />
      {has && !drawing && (
        <img src={`/api/v1/settings/debt-docs/signature?v=${version}`} alt="подпись руководителя" className="h-24 w-auto self-start rounded border border-slate-300 bg-white" />
      )}
      {drawing ? (
        <SignaturePad busy={busy} doneLabel="Сохранить подпись" hint="Распишитесь мышью или пером в белом поле" onDone={save} />
      ) : (
        <div className="flex flex-wrap gap-2">
          <Button variant="secondary" disabled={busy} onClick={() => setDrawing(true)}>
            {has ? 'Нарисовать заново' : 'Нарисовать подпись'}
          </Button>
          <label className="inline-flex cursor-pointer items-center rounded-md border border-slate-300 px-3 py-2 text-sm">
            Загрузить PNG
            <input type="file" accept="image/png" className="hidden" data-testid="owner-signature-file" onChange={(e) => upload(e.target.files?.[0])} />
          </label>
          {has && (
            <Button variant="secondary" disabled={busy} onClick={remove}>
              Убрать
            </Button>
          )}
        </div>
      )}
    </Card>
  )
}

/** Другие документы точки (задача 017): договоры с реквизитами точки и подписью руководителя —
 *  правятся здесь и печатаются отсюда в двух экземплярах. */
function ExtraDocsCard({
  docs,
  seller,
  hasSignature,
  onChange,
}: {
  docs: ExtraDoc[]
  seller: Seller
  hasSignature: boolean
  onChange: (d: ExtraDoc[]) => void
}) {
  const [open, setOpen] = useState<number | null>(null)
  const [preview, setPreview] = useState(false)
  const [sig, setSig] = useState<string | null>(null)
  useEffect(() => {
    let alive = true
    if (hasSignature) void pngDataUrl('/settings/debt-docs/signature').then((p) => alive && setSig(p))
    else setSig(null)
    return () => {
      alive = false
    }
  }, [hasSignature])
  const set = (i: number, patch: Partial<ExtraDoc>) => onChange(docs.map((d, j) => (j === i ? { ...d, ...patch } : d)))
  const add = () => {
    onChange([...docs, { title: 'Новый документ', text: '' }])
    setOpen(docs.length)
    setPreview(false)
  }
  const remove = (i: number) => {
    if (!window.confirm(`Удалить «${docs[i].title}»? Удаление сохранится кнопкой «Сохранить».`)) return
    onChange(docs.filter((_, j) => j !== i))
    setOpen(null)
  }
  return (
    <Card className="flex flex-col gap-3" data-testid="extra-docs">
      <CardTitle
        actions={
          <Button variant="secondary" onClick={add}>
            Добавить документ
          </Button>
        }
      >
        Другие документы
        <span className="block text-xs font-normal text-slate-500">
          Договоры точки: реквизиты и подпись руководителя ставятся сами, данные второй стороны впишите в текст или от руки. «Печать» — два экземпляра, каждый с
          нового листа.
        </span>
      </CardTitle>
      {docs.length === 0 && <div className="text-sm text-slate-500">Документов нет — «Добавить документ».</div>}
      {docs.map((d, i) => (
        <div key={i} className="flex flex-col gap-2 rounded-md border border-slate-200 p-3">
          <div className="flex flex-wrap items-center gap-2">
            <button type="button" className="mr-auto text-left font-medium text-sky-700 hover:underline" onClick={() => setOpen(open === i ? null : i)}>
              {open === i ? '▾' : '▸'} {d.title || 'Без названия'}
            </button>
            <Button variant="secondary" onClick={() => printExtraDoc(d, seller, sig)}>
              Печать
            </Button>
            <Button variant="secondary" onClick={() => remove(i)}>
              Удалить
            </Button>
          </div>
          {open === i && (
            <>
              <Field label="Название">
                <input value={d.title} onChange={(e) => set(i, { title: e.target.value })} />
              </Field>
              <div className="flex flex-wrap gap-2 text-sm">
                <Button variant={preview ? 'secondary' : 'primary'} onClick={() => setPreview(false)}>
                  Текст
                </Button>
                <Button variant={preview ? 'primary' : 'secondary'} onClick={() => setPreview(true)}>
                  Предпросмотр
                </Button>
                <Button
                  variant="secondary"
                  onClick={() =>
                    set(i, {
                      text: DEFAULT_EXTRA.find((x) => x.title === d.title)?.text ?? d.text,
                    })
                  }
                  disabled={!DEFAULT_EXTRA.some((x) => x.title === d.title)}
                >
                  Вернуть исходный текст
                </Button>
              </div>
              {preview ? (
                <div
                  className="overflow-x-auto rounded-md border border-slate-300 p-6 font-serif text-sm leading-relaxed whitespace-pre-wrap shadow-inner [&_.sig]:inline-block [&_.sig]:w-40 [&_.sig_img]:h-12"
                  style={{ background: '#fff', color: '#000' }}
                  // Текст экранируется в renderTemplate; подпись — проверенная картинка data:image/png.
                  dangerouslySetInnerHTML={{
                    __html: extraDocHtml(d.text, seller, sig),
                  }}
                />
              ) : (
                <>
                  <div className="text-xs text-slate-500">
                    Подстановки:{' '}
                    {EXTRA_PLACEHOLDERS.map(([k, h]) => (
                      <code key={k} title={h} className="mr-1 rounded bg-slate-100 px-1">{`{{${k}}}`}</code>
                    ))}
                  </div>
                  <textarea className="min-h-[420px] font-mono text-xs" value={d.text} onChange={(e) => set(i, { text: e.target.value })} />
                </>
              )}
            </>
          )}
        </div>
      ))}
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
        act_period: loaded.data.act_period ?? 'all',
        extra: loaded.data.extra ?? DEFAULT_EXTRA,
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
        act_period: form.act_period ?? 'all',
        extra: form.extra ?? DEFAULT_EXTRA,
      })
      toast('Сохранено')
    })

  if (!form) return loaded.error ? <ErrorBox error={loaded.error} /> : <Loading />

  return (
    <div className="flex flex-col gap-4">
      <PageHeader title="Документы" />
      <Card className="flex flex-col gap-3">
        <h2 className="text-base font-semibold">Реквизиты точки</h2>
        <div className="text-xs text-slate-500">Печатаются в расписке, накладной и акте сверки. Пустое поле печатается линией, чтобы вписать от руки.</div>
        <div className="grid gap-3 sm:grid-cols-2">
          {SELLER_FIELDS.map(([key, label, placeholder]) => (
            <Field key={key} label={label}>
              <input
                value={form.seller[key]}
                placeholder={placeholder}
                onChange={(e) =>
                  setForm({
                    ...form,
                    seller: { ...form.seller, [key]: e.target.value },
                  })
                }
              />
            </Field>
          ))}
        </div>
      </Card>

      <OwnerSignatureCard has={Boolean(loaded.data?.has_owner_signature)} onChanged={loaded.reload} />

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

      <ExtraDocsCard
        docs={form.extra ?? DEFAULT_EXTRA}
        seller={form.seller}
        hasSignature={Boolean(loaded.data?.has_owner_signature)}
        onChange={(extra) => setForm({ ...form, extra })}
      />

      <Card className="flex flex-col gap-3">
        <h2 className="text-base font-semibold">Акт сверки</h2>
        <Field label="Период акта по умолчанию" hint="С каким периодом открывается акт сверки в карточке клиента и поставщика; на экране его можно поменять">
          <select
            data-testid="act-default-period"
            value={form.act_period ?? 'all'}
            onChange={(e) => setForm({ ...form, act_period: e.target.value as ActPeriod })}
          >
            {ACT_PERIODS.map((p) => (
              <option key={p.key} value={p.key}>
                {p.label}
              </option>
            ))}
          </select>
        </Field>
      </Card>

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

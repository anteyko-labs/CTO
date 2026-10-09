// Реквизиты точки и тексты документов о долге (SPEC-10). Правит владелец.
import { useEffect, useState } from 'react'
import { Button, Card, ErrorBox, Field, Loading, PageHeader, toast } from '../components/ui'
import { get, put } from '../lib/api'
import { DEFAULT_COMPANY, DEFAULT_PERSON, PLACEHOLDERS, type DebtDocSettings, type Seller } from '../lib/debtDocs'
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

  const template = (key: 'person' | 'company', title: string, hint: string, fallback: string) => (
    <Card className="flex flex-col gap-2">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div>
          <h2 className="font-semibold">{title}</h2>
          <div className="text-xs text-slate-500">{hint}</div>
        </div>
        <Button variant="secondary" onClick={() => setForm({ ...form, [key]: fallback })}>
          Вернуть исходный текст
        </Button>
      </div>
      <textarea
        className="min-h-[360px] font-mono text-xs"
        value={form[key] ?? ''}
        onChange={(e) => setForm({ ...form, [key]: e.target.value })}
      />
    </Card>
  )

  return (
    <div className="flex flex-col gap-4">
      <PageHeader title="Документы о долге" />
      <Card className="flex flex-col gap-3">
        <h2 className="font-semibold">Реквизиты точки</h2>
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

      {template('person', 'Физическое лицо: расписка', 'Печатается в двух экземплярах — покупателю и точке', DEFAULT_PERSON)}
      {template('company', 'Юридическое лицо: накладная с отсрочкой оплаты', 'Один экземпляр с подписями и печатями обеих сторон', DEFAULT_COMPANY)}

      <Card>
        <h2 className="mb-2 font-semibold">Подстановки</h2>
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

      <ErrorBox error={error} />
      <div className="flex justify-end">
        <Button disabled={busy} onClick={save}>
          Сохранить
        </Button>
      </div>
    </div>
  )
}

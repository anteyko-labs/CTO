// Телеграм-бот владельца: привязка чата одноразовым кодом (SPEC-18, ADR-050); ссылкой и QR (ADR-121).
import { useEffect, useState } from 'react'
import QRCode from 'qrcode'
import { Button, Card, CardTitle, ErrorBox, Field, Loading, PageHeader, toast } from '../components/ui'
import { del, get, post, put } from '../lib/api'
import { formatDateTime } from '../lib/format'
import { useAction, useLoad } from '../lib/hooks'

interface Status {
  enabled: boolean
  linked: boolean
  linked_at: string | null
}

/** Процент баллов с покупки: «0,5» ↔ 50 сотых процента (SPEC-19). */
function LoyaltyRate() {
  const st = useLoad(() => get<{ rate_bp: number }>('/settings/loyalty'), [])
  const [text, setText] = useState<string | null>(null)
  const act = useAction()
  const m = text === null ? null : /^(\d{1,2})(?:[.,](\d{1,2}))?$/.exec(text.trim())
  const bp = m ? Number(m[1]) * 100 + Number((m[2] ?? '').padEnd(2, '0')) : null
  const shown = (v: number) => String(v / 100).replace('.', ',')
  if (!st.data) return <ErrorBox error={st.error} />
  return (
    <Card className="flex flex-wrap items-end gap-3 text-sm">
      <div className="min-w-0 flex-1 text-slate-700">
        <CardTitle>Баллы клиентам</CardTitle>
        Клиенты, подключившиеся к боту по номеру, получают {shown(st.data.rate_bp)} % от оплаченного деньгами баллами (1 балл = 1 сом). Списать
        баллы — на кассе «Услуги → Скидка баллами». Списанные баллы уменьшают чистую прибыль.
      </div>
      {text === null ? (
        <Button variant="secondary" onClick={() => setText(shown(st.data!.rate_bp))}>
          Изменить процент
        </Button>
      ) : (
        <>
          <Field label="Процент, %" hint="От 0 до 10">
            <input autoFocus inputMode="decimal" className="w-24" value={text} onChange={(e) => setText(e.target.value)} />
          </Field>
          <Button variant="secondary" onClick={() => setText(null)}>
            Отмена
          </Button>
          <Button
            disabled={act.busy || bp === null || bp > 1000}
            onClick={() =>
              void act.run(async () => {
                await put('/settings/loyalty', { rate_bp: bp })
                setText(null)
                st.reload()
                toast('Процент сохранён')
              })
            }
          >
            Сохранить
          </Button>
        </>
      )}
      <ErrorBox error={act.error} />
    </Card>
  )
}

interface Usage {
  questions: number
  input_tokens: number
  output_tokens: number
}

interface QuestionsUsage {
  enabled: boolean
  daily_limit: number | null
  today: Usage
  month: Usage
}

/** Ссылка, открывающая бота сразу с кодом привязки (SPEC-36 §7). */
function startLink(username: string, code: string): string {
  return `https://t.me/${encodeURIComponent(username)}?start=${encodeURIComponent(code)}`
}

/** Кнопка «Открыть бота» и QR той же ссылки — навести телефон владельца. */
function BotStartLink({ link }: { link: string }) {
  const [qr, setQr] = useState<string | null>(null)
  useEffect(() => {
    let alive = true
    QRCode.toDataURL(link, { margin: 1, width: 192, errorCorrectionLevel: 'M' })
      .then((url) => alive && setQr(url))
      .catch(() => alive && setQr(null))
    return () => {
      alive = false
    }
  }, [link])
  return (
    <div className="flex flex-wrap items-center gap-4">
      {qr && <img src={qr} alt="QR-код ссылки на бота" data-testid="bot-qr" className="h-48 w-48 rounded border border-slate-200" />}
      <div className="flex flex-col gap-2">
        <a
          href={link}
          target="_blank"
          rel="noopener noreferrer"
          className="inline-flex min-h-[42px] items-center justify-center rounded-md bg-sky-600 px-4 py-2 text-sm font-medium text-white hover:bg-sky-700"
        >
          Открыть бота
        </a>
        <div className="text-slate-600">На телефоне — наведите камеру на QR, бот откроется с кодом: нажмите «Запустить».</div>
      </div>
    </div>
  )
}

const count = (n: number) => n.toLocaleString('ru-RU')

/** Вопросы владельца боту обычным текстом: включены ли и расход (SPEC-21). */
function BotQuestions() {
  const st = useLoad(() => get<QuestionsUsage>('/settings/telegram/questions'), [])
  const u = st.data
  if (!u) return <ErrorBox error={st.error} />
  return (
    <Card className="flex flex-col gap-2 text-sm text-slate-700">
      <CardTitle>Вопросы боту текстом</CardTitle>
      {u.enabled ? (
        <div className="-mt-2">
          Включены: в привязанном чате нажмите «💬 Спросить» и спросите обычным текстом — «сколько заработали за неделю», «кто больше
          всех должен», «что кончается». Бот отвечает только о делах точки. Режим закрывается кнопкой «✖ Закончить» или через 10 минут
          без сообщений. Цифры берутся из тех же отчётов, что в программе. Не больше {u.daily_limit ?? '—'} вопросов в сутки.
        </div>
      ) : (
        <div className="-mt-2">
          Выключены: на сервере не задан ключ сервиса разбора вопросов
          <code className="mx-1 rounded bg-slate-100 px-1">ASSISTANT_API_KEY</code>. Кнопки и команды бота работают и без него.
        </div>
      )}
      <div className="grid grid-cols-2 gap-2 sm:max-w-md">
        {(
          [
            ['Сегодня', u.today],
            ['За месяц', u.month],
          ] as const
        ).map(([title, x]) => (
          <div key={title} className="rounded border border-slate-200 p-2">
            <div className="font-medium">{title}</div>
            <div>вопросов: {count(x.questions)}</div>
            <div className="text-slate-500">
              токенов: {count(x.input_tokens)} на вход, {count(x.output_tokens)} на выход
            </div>
          </div>
        ))}
      </div>
    </Card>
  )
}

export default function TelegramSettings() {
  const st = useLoad(() => get<Status>('/settings/telegram'), [])
  const bot = useLoad(() => get<{ username: string | null }>('/telegram/bot'), [])
  const [code, setCode] = useState<{ code: string; expires_at: string } | null>(null)
  const act = useAction()
  const s = st.data

  return (
    <div className="flex flex-col gap-4">
      <PageHeader title="Телеграм и баллы" />
      <ErrorBox error={st.error ?? act.error} />
      {!s ? (
        <Loading />
      ) : !s.enabled ? (
        <Card className="text-sm text-slate-700">
          <CardTitle>Бот владельца</CardTitle>
          Бот выключен: на сервере не задан ключ бота. Создайте бота у @BotFather в Телеграме, ключ впишите в настройку сервера
          <code className="mx-1 rounded bg-slate-100 px-1">TELEGRAM_BOT_TOKEN</code>и перезапустите сервер.
        </Card>
      ) : s.linked ? (
        <Card className="flex flex-wrap items-center justify-between gap-3 text-sm">
          <div>
            <CardTitle>Телеграм привязан</CardTitle>
            <div className="text-slate-600">
              С {s.linked_at ? formatDateTime(s.linked_at) : '—'} сюда приходят важные события: закрытие смены и сдача кассы, недостача, возвраты,
              сторно, смена цен, долги. Кнопки внизу чата: «Сегодня», «Смена», «Долги».
            </div>
          </div>
          <Button
            variant="secondary"
            disabled={act.busy}
            onClick={() =>
              void act.run(async () => {
                await del('/settings/telegram')
                st.reload()
                toast('Телеграм отвязан')
              })
            }
          >
            Отвязать
          </Button>
        </Card>
      ) : (
        <Card className="flex flex-col gap-3 text-sm">
          <CardTitle>Привязать Телеграм</CardTitle>
          <div className="-mt-3 text-slate-700">
            Команды и уведомления — только для владельца; клиенты, поделившись номером, видят в боте свою масляную книжку. Получите код и отправьте его боту — после этого уведомления будут приходить в этот чат.
          </div>
          {code ? (
            <div className="flex flex-col gap-2">
              <div className="text-3xl font-bold tracking-widest">{code.code}</div>
              <div className="text-slate-600">
                Откройте бота и отправьте: <code className="rounded bg-slate-100 px-1">/start {code.code}</code> — код действует до{' '}
                {formatDateTime(code.expires_at)}.
              </div>
              {bot.data?.username && <BotStartLink link={startLink(bot.data.username, code.code)} />}
              <div>
                <Button variant="secondary" onClick={() => st.reload()}>
                  Я отправил — проверить
                </Button>
              </div>
            </div>
          ) : (
            <div>
              <Button
                disabled={act.busy}
                onClick={() =>
                  void act.run(async () => {
                    setCode(await post<{ code: string; expires_at: string }>('/settings/telegram/code', {}))
                  })
                }
              >
                Получить код
              </Button>
            </div>
          )}
        </Card>
      )}
      {s?.enabled && <BotQuestions />}
      {s?.enabled && <LoyaltyRate />}
    </div>
  )
}

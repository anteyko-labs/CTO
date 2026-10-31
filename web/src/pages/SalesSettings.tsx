// «Настройки → Продажи» (задача 003): кто задаёт цену новой услуги. Только владелец; изменение —
// в журнал действий `settings.sales` (сервер). О каждой услуге, заведённой администратором, владелец
// узнаёт в «Уведомлениях» (событие «Администратор завёл услугу», выключается в «Уведомлениях в бот»).
import { Link } from 'react-router-dom'
import { Card, CardTitle, ErrorBox, Loading, PageHeader, toast } from '../components/ui'
import { get, put } from '../lib/api'
import { useAction, useLoad } from '../lib/hooks'

interface SalesSettings {
  oil_change_master_fee_tyiyn: number
  service_price_owner_only: boolean
}

export default function SalesSettingsPage() {
  const st = useLoad(() => get<SalesSettings>('/settings/sales'), [])
  const act = useAction()
  const s = st.data

  const save = (patch: Partial<SalesSettings>, done: string) =>
    void act.run(async () => {
      st.setData((prev) => prev && { ...prev, ...patch })
      try {
        st.setData(await put<SalesSettings>('/settings/sales', patch))
        toast(done)
      } catch (e) {
        st.reload()
        throw e
      }
    })

  return (
    <div>
      <PageHeader title="Продажи" subtitle="Услуги и их цены" back={<Link to="/settings">← Настройки</Link>} />
      <ErrorBox error={st.error ?? act.error} />
      {!s ? (
        <Loading />
      ) : (
        <div className="space-y-4">
          <Card data-testid="sales-service-price">
            <CardTitle>Цена новой услуги</CardTitle>
            <label className="inline-flex items-center gap-2 text-sm">
              <input
                id="sales-service-price-owner-only"
                type="checkbox"
                className="h-4 w-4"
                checked={s.service_price_owner_only}
                disabled={act.busy}
                onChange={(e) =>
                  save(
                    { service_price_owner_only: e.target.checked },
                    e.target.checked ? 'Цену новой услуги задаёт только владелец' : 'Цену новой услуги задаёт и администратор',
                  )
                }
              />
              Цену новой услуги задаёт только владелец
            </label>
            <p className="mt-2 text-sm text-slate-500">
              Включено — администратор заводит услугу с ценой 0, цену ставите вы в «Услугах». Администратор с правом «Цены товаров и услуг» задаёт цену и
              так. О каждой услуге, заведённой администратором, приходит уведомление с ценой.
            </p>
          </Card>
        </div>
      )}
    </div>
  )
}

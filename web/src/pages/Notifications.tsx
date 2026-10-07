import { useEffect } from 'react'
import { Badge, Card, Empty, ErrorBox, Loading, PageHeader, Table } from '../components/ui'
import { get, post } from '../lib/api'
import { formatDateTime } from '../lib/format'
import { useLoad } from '../lib/hooks'
import type { NotificationsOut } from '../lib/types'

/** Что происходило без владельца: цены, возвраты, сторно, долги, доступы. */
export default function Notifications() {
  const data = useLoad(() => get<NotificationsOut>('/notifications'), [])

  // Открыли экран — значит прочитали.
  useEffect(() => {
    if (data.data) void post('/notifications/seen').catch(() => undefined)
  }, [data.data])

  const items = data.data?.items ?? []

  return (
    <div>
      <PageHeader title="Уведомления" />
      <Card>
        <ErrorBox error={data.error} />
        {data.loading && !data.data ? (
          <Loading />
        ) : items.length === 0 ? (
          <Empty>Пока ничего важного не происходило</Empty>
        ) : (
          <Table head={['Когда', 'Что', 'Подробности', 'Кто']}>
            {items.map((n) => (
              <tr key={n.id} className={n.new ? 'bg-sky-50/60' : ''}>
                <td className="whitespace-nowrap px-2 py-2">{formatDateTime(n.at)}</td>
                <td className="px-2 py-2">
                  <span className="font-medium">{n.title}</span>
                  {n.new && (
                    <span className="ml-2">
                      <Badge tone="sky">новое</Badge>
                    </span>
                  )}
                </td>
                <td className="px-2 py-2 text-slate-600">{n.details || '—'}</td>
                <td className="whitespace-nowrap px-2 py-2">{n.user_name ?? '—'}</td>
              </tr>
            ))}
          </Table>
        )}
      </Card>
    </div>
  )
}

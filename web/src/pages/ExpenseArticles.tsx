// Статьи расходов: добавить, переименовать, «только владелец», скрыть (SPEC-06). Только владелец.
import { useState, type FormEvent } from 'react'
import { Badge, Button, Card, Checkbox, Empty, ErrorBox, Field, Loading, Modal, PageHeader, RowMenu, Table, toast } from '../components/ui'
import { get, patch, post } from '../lib/api'
import { useAction, useLoad } from '../lib/hooks'
import type { ExpenseArticle } from '../lib/types'

export default function ExpenseArticles() {
  const list = useLoad(() => get<ExpenseArticle[]>('/expense-articles'), [])
  const [adding, setAdding] = useState(false)
  const [form, setForm] = useState({ name: '', owner_only: false })
  const [renaming, setRenaming] = useState<{ a: ExpenseArticle; name: string } | null>(null)
  const act = useAction()

  const create = (e: FormEvent) => {
    e.preventDefault()
    void act.run(async () => {
      await post<ExpenseArticle>('/expense-articles', { name: form.name, owner_only: form.owner_only })
      setForm({ name: '', owner_only: false })
      setAdding(false)
      list.reload()
      toast('Статья добавлена')
    })
  }

  const update = (a: ExpenseArticle, body: Partial<Pick<ExpenseArticle, 'name' | 'owner_only' | 'active'>>, done: string) =>
    void act.run(async () => {
      await patch<ExpenseArticle>(`/expense-articles/${a.id}`, body)
      setRenaming(null)
      list.reload()
      toast(done)
    })

  return (
    <div>
      <PageHeader
        title="Статьи расходов"
        subtitle="Скрытая статья пропадает из выбора, прошлые расходы по ней остаются в отчётах"
        actions={<Button onClick={() => setAdding(true)}>Добавить статью</Button>}
      />
      <Card>
        <ErrorBox error={list.error ?? act.error} />
        {list.loading && !list.data ? (
          <Loading />
        ) : !list.data?.length ? (
          <Empty>Статей пока нет</Empty>
        ) : (
          <Table head={['Статья', 'Кто видит', 'Статус', '']}>
            {list.data.map((a) => (
              <tr key={a.id} className={a.active ? '' : 'text-slate-400'}>
                <td className="px-2 py-2 font-medium">{a.name}</td>
                <td className="px-2 py-2">{a.owner_only ? <Badge tone="amber">только владелец</Badge> : 'все'}</td>
                <td className="px-2 py-2">{a.active ? <Badge tone="green">в выборе</Badge> : <Badge>скрыта</Badge>}</td>
                <td className="overflow-visible! px-2 py-1 text-right">
                  <RowMenu
                    items={[
                      { label: 'Переименовать', onClick: () => setRenaming({ a, name: a.name }) },
                      a.owner_only
                        ? { label: 'Показывать администратору', onClick: () => update(a, { owner_only: false }, 'Статья видна администратору') }
                        : { label: 'Только владелец', onClick: () => update(a, { owner_only: true }, 'Статья только владельцу') },
                      a.active
                        ? { label: 'Скрыть', danger: true, onClick: () => update(a, { active: false }, 'Статья скрыта') }
                        : { label: 'Вернуть в выбор', onClick: () => update(a, { active: true }, 'Статья снова в выборе') },
                    ]}
                  />
                </td>
              </tr>
            ))}
          </Table>
        )}
        <p className="mt-3 text-xs text-slate-500">
          «Только владелец» — администратор статью не видит и расход по ней не записывает (например, личные расходы). Личные расходы
          уменьшают прибыль, зарплата статьёй не проводится.
        </p>
      </Card>

      {adding && (
        <Modal title="Новая статья расходов" onClose={() => setAdding(false)}>
          <form onSubmit={create} className="flex flex-col gap-3">
            <Field label="Название" required>
              <input autoFocus maxLength={100} value={form.name} onChange={(e) => setForm({ ...form, name: e.target.value })} />
            </Field>
            <Checkbox label="Только владелец" checked={form.owner_only} onChange={(v) => setForm({ ...form, owner_only: v })} />
            <ErrorBox error={act.error} />
            <div className="flex justify-end gap-2">
              <Button variant="secondary" onClick={() => setAdding(false)}>
                Отмена
              </Button>
              <Button type="submit" disabled={act.busy || !form.name.trim()}>
                Добавить
              </Button>
            </div>
          </form>
        </Modal>
      )}

      {renaming && (
        <Modal title="Переименовать статью" onClose={() => setRenaming(null)}>
          <div className="flex flex-col gap-3">
            <Field label="Название" required>
              <input autoFocus maxLength={100} value={renaming.name} onChange={(e) => setRenaming({ ...renaming, name: e.target.value })} />
            </Field>
            <ErrorBox error={act.error} />
            <div className="flex justify-end gap-2">
              <Button variant="secondary" onClick={() => setRenaming(null)}>
                Отмена
              </Button>
              <Button
                disabled={act.busy || !renaming.name.trim() || renaming.name.trim() === renaming.a.name}
                onClick={() => update(renaming.a, { name: renaming.name.trim() }, 'Статья переименована')}
              >
                Сохранить
              </Button>
            </div>
          </div>
        </Modal>
      )}
    </div>
  )
}

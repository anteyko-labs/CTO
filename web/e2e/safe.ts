// «Сейф владельца» для проверок: приходы для склада оплачиваются из него — «из своих, мимо касс»
// больше нет (ADR-079).
import { expect, type APIRequestContext } from '@playwright/test'

export async function ownerSafe(request: APIRequestContext): Promise<string> {
  const res = await request.get('/api/v1/cash/accounts')
  expect(res.ok(), await res.text()).toBeTruthy()
  const list = (await res.json()) as { id: string; name: string; kind: string; owner_only: boolean }[]
  const safe = list.find((a) => a.kind === 'safe' && a.owner_only) ?? list.find((a) => a.kind === 'safe')
  expect(safe, 'нет сейфа владельца').toBeTruthy()
  return safe!.id
}

/** Черновик прихода и оприходования свой у входа (ADR-190): у тестов один владелец, форма начинается с чистого листа. */
export async function discardEntryDrafts(request: APIRequestContext): Promise<void> {
  for (const kind of ['receipt', 'stock_in']) {
    const res = await request.post(`/api/v1/entry-drafts/${kind}/discard`, { headers: { 'X-Device-Id': '00000000-0000-4000-8000-00000000e2d0' } })
    expect(res.ok(), await res.text()).toBeTruthy()
  }
}

// Объединение карточек (задача 043): что станет с долгом после слияния.

export interface MergeCard {
  id: string
  balance_tyiyn: number
}

/** Общий долг после объединения: основная плюс все выбранные (аванс — со знаком минус). */
export function mergedBalance(main: MergeCard, picked: MergeCard[]): number {
  return picked.filter((p) => p.id !== main.id).reduce((sum, p) => sum + p.balance_tyiyn, main.balance_tyiyn)
}

/** Человек, влитый в фирму, становится её работником (агентом). */
export function becomesAgents(finalKind: 'person' | 'company', picked: { kind: string; name: string }[]): string[] {
  return finalKind === 'company' ? picked.filter((p) => p.kind === 'person').map((p) => p.name) : []
}

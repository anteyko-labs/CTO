/** Что показать вместо «Начать ревизию»: новую ревизию начинает только владелец, остальные
 *  присоединяются к начатой (задача 019, ADR-186). Сервер проверяет сам — это только экран. */
export type StartMode =
  | { kind: "start" }
  | { kind: "join"; drafts: { id: string; number: number; label: string }[] }
  | { kind: "wait" };

export const WAIT_OWNER =
  "Ревизию начинает владелец. Когда начнёт — она появится здесь, откройте её и считайте.";

interface DraftHead {
  id: string;
  number: number;
  status: "draft" | "posted" | "cancelled";
  category_name: string | null;
}

export function startMode(owner: boolean, heads: DraftHead[]): StartMode {
  if (owner) return { kind: "start" };
  const drafts = heads
    .filter((h) => h.status === "draft")
    .map((h) => ({
      id: h.id,
      number: h.number,
      label: `Присоединиться к ревизии № ${h.number} · ${h.category_name ?? "весь склад"}`,
    }));
  return drafts.length > 0 ? { kind: "join", drafts } : { kind: "wait" };
}

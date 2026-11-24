// Чек юрлица под ответственность кассира (задача 046, ADR-213): какие отказы сервера можно
// провести с пояснением кассира и какое пояснение принимается. Сервер проверяет то же самое.

/** Отказы, которые кассир может провести под свою ответственность: нет ответа фирмы. */
export const RESPONSIBILITY_CODES = [
  "approval_required",
  "contact_pending",
  "vehicle_pending",
] as const;

/** Показывать ли кнопку «Провести под мою ответственность» на этот отказ. */
export function canTakeResponsibility(
  code: string | null | undefined,
): boolean {
  return (RESPONSIBILITY_CODES as readonly string[]).includes(code ?? "");
}

export const NOTE_MIN = 5;
export const NOTE_MAX = 300;

/** Ошибка поля «Почему проводите» или null, если пояснение подходит. */
export function noteError(note: string): string | null {
  const n = [...note.trim()].length;
  if (n === 0) return "Напишите, почему проводите";
  if (n < NOTE_MIN || n > NOTE_MAX)
    return `От ${NOTE_MIN} до ${NOTE_MAX} знаков`;
  return null;
}

/** Причины отказа без повторов — списком в окне. */
export function addReason(reasons: string[], reason: string): string[] {
  const r = reason.trim();
  return !r || reasons.includes(r) ? reasons : [...reasons, r];
}

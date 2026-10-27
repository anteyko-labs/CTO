// Фото накладной: подготовка выбранных файлов и загрузка к накладной (SPEC-29 §3, ADR-080).
import { newOpId, qs, randomId, upload } from './api'
import { prepareFile, type Prepared } from './photo'

/** Файл, ждущий загрузки: `opId` один на файл — повтор после обрыва не заведёт второй. */
export interface PendingPhoto extends Prepared {
  key: string
  opId: string
  url: string
}

/** Подготовить выбранные файлы: снимки сжимаются, PDF — как есть. */
export async function preparePhotos(files: FileList | File[]): Promise<PendingPhoto[]> {
  const out: PendingPhoto[] = []
  for (const f of Array.from(files)) {
    const p = await prepareFile(f)
    out.push({ ...p, key: randomId(), opId: newOpId(), url: URL.createObjectURL(p.blob) })
  }
  return out
}

/** Загрузить фото к проведённой накладной; вернуть, сколько не ушло. */
export async function uploadPhotos(receiptId: string, photos: PendingPhoto[]): Promise<string[]> {
  const failed: string[] = []
  for (const p of photos) {
    try {
      await upload(`/receipts/${receiptId}/files${qs({ op_id: p.opId, name: p.name })}`, p.blob, p.type)
    } catch (e) {
      failed.push(`${p.name}: ${e instanceof Error ? e.message : 'ошибка'}`)
    }
  }
  return failed
}


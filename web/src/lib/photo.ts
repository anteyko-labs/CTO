// Фото накладной перед загрузкой (SPEC-29 §3, ADR-080): снимок с телефона уменьшается до
// 2000 точек по длинной стороне и сжимается в JPEG; PDF и маленькие снимки уходят как есть.

/** Предел сервера на один файл. */
export const MAX_UPLOAD_BYTES = 5 * 1024 * 1024
/** Длинная сторона снимка после уменьшения. */
export const MAX_SIDE = 2000

/** Размер после уменьшения: длинная сторона не больше `max`, пропорции сохраняются, целые точки. */
export function fitSize(width: number, height: number, max = MAX_SIDE): { width: number; height: number } {
  if (width <= 0 || height <= 0) return { width: 0, height: 0 }
  const side = Math.max(width, height)
  if (side <= max) return { width, height }
  return { width: Math.max(1, Math.round((width * max) / side)), height: Math.max(1, Math.round((height * max) / side)) }
}

/** Какие файлы принимает сервер. */
export const ACCEPTED_TYPES = ['image/jpeg', 'image/png', 'image/webp', 'application/pdf']

export interface Prepared {
  blob: Blob
  type: string
  name: string
}

/** Снимок — уменьшить и сжать; PDF — как есть. Не тот тип или больше 5 МБ после сжатия — ошибка. */
export async function prepareFile(file: File): Promise<Prepared> {
  if (file.type === 'application/pdf') {
    if (file.size > MAX_UPLOAD_BYTES) throw new Error(`«${file.name}» больше 5 МБ`)
    return { blob: file, type: file.type, name: file.name }
  }
  if (!file.type.startsWith('image/')) throw new Error(`«${file.name}»: нужен снимок или PDF`)
  try {
    const bitmap = await createImageBitmap(file)
    const size = fitSize(bitmap.width, bitmap.height)
    const canvas = document.createElement('canvas')
    canvas.width = size.width
    canvas.height = size.height
    const ctx = canvas.getContext('2d')
    if (!ctx) throw new Error('нет холста')
    ctx.drawImage(bitmap, 0, 0, size.width, size.height)
    bitmap.close()
    const blob = await new Promise<Blob | null>((resolve) => canvas.toBlob(resolve, 'image/jpeg', 0.82))
    if (blob && blob.size <= MAX_UPLOAD_BYTES) {
      return { blob, type: 'image/jpeg', name: file.name.replace(/\.[^.]+$/, '') + '.jpg' }
    }
  } catch {
    // Браузер не смог прочитать снимок — пробуем отправить как есть.
  }
  if (!ACCEPTED_TYPES.includes(file.type)) throw new Error(`«${file.name}»: нужен снимок JPEG, PNG, WEBP или PDF`)
  if (file.size > MAX_UPLOAD_BYTES) throw new Error(`«${file.name}» больше 5 МБ`)
  return { blob: file, type: file.type, name: file.name }
}

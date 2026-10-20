// Чтение первого листа .xlsx в браузере без сторонних библиотек: xlsx — это zip с XML внутри,
// распаковка — встроенным DecompressionStream('deflate-raw') (SPEC-20, ADR-056).

/** Файлы архива: имя → данные (сжатые) и способ сжатия. */
function zipEntries(buf: ArrayBuffer): Map<string, { method: number; data: Uint8Array }> {
  const view = new DataView(buf)
  const bytes = new Uint8Array(buf)
  // Конец центрального каталога — в последних 64 КБ файла.
  let eocd = -1
  for (let i = bytes.length - 22; i >= Math.max(0, bytes.length - 65_557); i--) {
    if (view.getUint32(i, true) === 0x06054b50) {
      eocd = i
      break
    }
  }
  if (eocd < 0) throw new Error('Это не файл Excel (.xlsx)')
  const count = view.getUint16(eocd + 10, true)
  let p = view.getUint32(eocd + 16, true)
  const out = new Map<string, { method: number; data: Uint8Array }>()
  const dec = new TextDecoder()
  for (let n = 0; n < count; n++) {
    if (view.getUint32(p, true) !== 0x02014b50) break
    const method = view.getUint16(p + 10, true)
    const size = view.getUint32(p + 20, true)
    const nameLen = view.getUint16(p + 28, true)
    const extraLen = view.getUint16(p + 30, true)
    const commentLen = view.getUint16(p + 32, true)
    const local = view.getUint32(p + 42, true)
    const name = dec.decode(bytes.subarray(p + 46, p + 46 + nameLen))
    const start = local + 30 + view.getUint16(local + 26, true) + view.getUint16(local + 28, true)
    out.set(name, { method, data: bytes.subarray(start, start + size) })
    p += 46 + nameLen + extraLen + commentLen
  }
  return out
}

async function inflate(e: { method: number; data: Uint8Array }): Promise<string> {
  if (e.method === 0) return new TextDecoder().decode(e.data)
  if (e.method !== 8) throw new Error('Неизвестное сжатие в файле Excel')
  const stream = new Blob([e.data as BlobPart]).stream().pipeThrough(new DecompressionStream('deflate-raw'))
  return new Response(stream).text()
}

/** Номер столбца по ссылке ячейки: «C12» → 2. */
export function columnIndex(ref: string): number {
  let n = 0
  for (const ch of ref) {
    const c = ch.charCodeAt(0)
    if (c < 65 || c > 90) break
    n = n * 26 + (c - 64)
  }
  return n - 1
}

/** Первый лист как массив строк (значения ячеек — текстом). */
export async function readXlsx(file: Blob): Promise<string[][]> {
  const zip = zipEntries(await file.arrayBuffer())
  const parser = new DOMParser()
  const shared: string[] = []
  const ss = zip.get('xl/sharedStrings.xml')
  if (ss) {
    const doc = parser.parseFromString(await inflate(ss), 'application/xml')
    for (const si of Array.from(doc.getElementsByTagName('si'))) {
      shared.push(Array.from(si.getElementsByTagName('t')).map((t) => t.textContent ?? '').join(''))
    }
  }
  const sheetName = [...zip.keys()].filter((k) => /^xl\/worksheets\/sheet\d+\.xml$/.test(k)).sort()[0]
  if (!sheetName) throw new Error('В файле нет листов')
  const sheet = zip.get(sheetName)
  if (!sheet) throw new Error('В файле нет листов')
  const doc = parser.parseFromString(await inflate(sheet), 'application/xml')
  const rows: string[][] = []
  for (const r of Array.from(doc.getElementsByTagName('row'))) {
    const row: string[] = []
    for (const c of Array.from(r.getElementsByTagName('c'))) {
      const i = columnIndex(c.getAttribute('r') ?? 'A')
      const t = c.getAttribute('t')
      const v = c.getElementsByTagName('v')[0]?.textContent ?? ''
      const value =
        t === 's' ? (shared[Number(v)] ?? '') : t === 'inlineStr' ? Array.from(c.getElementsByTagName('t')).map((x) => x.textContent ?? '').join('') : v
      while (row.length < i) row.push('')
      row[i] = cellText(value)
    }
    if (row.some((x) => x !== '')) rows.push(row)
  }
  return rows
}

/** Текст ячейки: служебные «_x000d_» Excel раскрываются, переводы строк и прочие управляющие
 *  знаки — пробелом, лишние пробелы убираются (иначе они попадали в название товара). */
export function cellText(v: string): string {
  return v
    .replace(/_x([0-9A-Fa-f]{4})_/g, (_, hex: string) => String.fromCharCode(parseInt(hex, 16)))
    // eslint-disable-next-line no-control-regex
    .replace(/[\u0000-\u001f\u007f]/g, ' ')
    .replace(/\s+/g, ' ')
    .trim()
}

/** Сумма из Excel («150.813», «1 200,5») в тыйынах, округление до тыйына целыми. */
export function somToTyiyn(text: string): number {
  const s = text.replace(/\s/g, '').replace(',', '.')
  const m = /^(\d+)(?:\.(\d+))?$/.exec(s)
  if (!m) return 0
  const frac = (m[2] ?? '').padEnd(3, '0')
  const cents = Number(m[1]) * 100 + Number(frac.slice(0, 2)) + (Number(frac[2]) >= 5 ? 1 : 0)
  return Number.isSafeInteger(cents) ? cents : 0
}

/** Объём тары из названия масла: «AZMOL 5W40 4Л» → 4000 мл, «200L» → 200 000, «0,946L» → 946.
 *  Число сразу после буквы — часть вязкости («5W30 L» — не 30 л); из нескольких — последнее. */
export function containerFromName(name: string): number | null {
  const all = [...name.matchAll(/(?<![\p{L}\d.,])(\d+(?:[.,]\d+)?)\s*(?:L|Л|l|л|LT|lt|литр)(?![a-zа-я])/giu)]
  const m = all[all.length - 1]
  if (!m) return null
  const [whole, frac = ''] = m[1].replace(',', '.').split('.')
  const ml = Number(whole) * 1000 + Number(frac.padEnd(3, '0').slice(0, 3))
  return ml > 0 && ml <= 1_000_000 ? ml : null
}

// Чтение выгруженного .xlsx в проверках (задача 003 Н13): zip без сжатия, как пишет
// web/src/lib/xlsxWrite.ts. Листы — строками текста, как readXlsx (web/src/lib/xlsx.ts).
import { readFileSync } from 'node:fs'

/** Файлы архива: имя → текст. Сжатые (не наши) файлы не читаются. */
export function unzipStore(bytes: Uint8Array): Map<string, string> {
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength)
  const out = new Map<string, string>()
  const dec = new TextDecoder()
  let p = 0
  while (p + 30 <= bytes.length && view.getUint32(p, true) === 0x04034b50) {
    if (view.getUint16(p + 8, true) !== 0) throw new Error('файл сжат: это не наша выгрузка')
    const size = view.getUint32(p + 18, true)
    const nameLen = view.getUint16(p + 26, true)
    const extra = view.getUint16(p + 28, true)
    const name = dec.decode(bytes.subarray(p + 30, p + 30 + nameLen))
    const start = p + 30 + nameLen + extra
    out.set(name, dec.decode(bytes.subarray(start, start + size)))
    p = start + size
  }
  return out
}

const unescape = (s: string) => s.replace(/&lt;/g, '<').replace(/&gt;/g, '>').replace(/&quot;/g, '"').replace(/&amp;/g, '&')

/** Строки листа: текст ячеек по порядку столбцов. */
export function sheetRows(xml: string): string[][] {
  const out: string[][] = []
  for (const r of xml.matchAll(/<row [^>]*>([\s\S]*?)<\/row>/g)) {
    const row: string[] = []
    for (const c of r[1].matchAll(/<c r="([A-Z]+)\d+"([^>]*?)(?:\/>|>([\s\S]*?)<\/c>)/g)) {
      let i = 0
      for (const ch of c[1]) i = i * 26 + (ch.charCodeAt(0) - 64)
      const body = c[3] ?? ''
      const v = /t="inlineStr"/.test(c[2]) ? [...body.matchAll(/<t[^>]*>([\s\S]*?)<\/t>/g)].map((m) => m[1]).join('') : (/<v>(.*?)<\/v>/.exec(body)?.[1] ?? '')
      while (row.length < i - 1) row.push('')
      row[i - 1] = unescape(v)
    }
    if (row.some((x) => x !== '')) out.push(row)
  }
  return out
}

/** Книга из файла: имена листов и их строки. */
export function readBook(path: string): { names: string[]; sheets: string[][][] } {
  const files = unzipStore(new Uint8Array(readFileSync(path)))
  const wb = files.get('xl/workbook.xml') ?? ''
  const names = [...wb.matchAll(/<sheet name="([^"]*)"/g)].map((m) => unescape(m[1]))
  const sheets = names.map((_, i) => sheetRows(files.get(`xl/worksheets/sheet${i + 1}.xml`) ?? ''))
  return { names, sheets }
}

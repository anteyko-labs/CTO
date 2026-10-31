// Запись .xlsx в браузере без сторонних библиотек (задача 003 Н13, ADR-{B10.3}; чтение — xlsx.ts,
// ADR-056). Файл — zip без сжатия (STORE) с CRC32 и минимальными XML-частями: книга, стили, листы.
// Числа пишутся строкой из целых (тыйыны, мл, штуки) без дробных вычислений (I1, I2): 16395200
// тыйынов → «163952.00». Текст — всегда текстом (inlineStr): коды «0888602305» и 13-значные
// штрихкоды не превращаются в числа. Шапка таблицы закреплена, у столбцов подобрана ширина.

/** Ячейка: строка — текст; типизированные — число Excel с форматом. */
export type Cell =
  | string
  | null
  | undefined
  /** Деньги в тыйынах → сомы с двумя знаками, формат «# ##0,00». */
  | { money: number }
  /** Целое количество; `scale` — сколько знаков дробной части (мл → л: 3). */
  | { qty: number; scale?: number }
  /** Целое число (номер, штуки) — формат «# ##0». */
  | { int: number }
  /** День «ГГГГ-ММ-ДД» → настоящая дата Excel. */
  | { date: string }

export interface SheetRow {
  cells: Cell[]
  /** Уровень вложенности строки (0 — верхний): отступ в первом столбце и группировка Excel. */
  level?: number
  bold?: boolean
}

export interface Column {
  label: string
  /** Ширина в знаках; без неё — по содержимому. */
  width?: number
}

export interface Sheet {
  name: string
  columns: Column[]
  rows: (Cell[] | SheetRow)[]
  /** Строки над шапкой: название, период, филиал, когда выгружено. */
  title?: string[]
  /** Строки под таблицей (отбор, предупреждение). */
  footer?: string[]
  /** Автофильтр по шапке (по умолчанию — да; для отчёта-дерева — нет). */
  filter?: boolean
  /** Предупреждение для человека: показывается при выгрузке и пишется под таблицей. */
  warning?: string
}

// ---------- Числа из целых ----------

/** Целое со `scale` знаками дробной части строкой: (−12050, 2) → «-120.50». Без float. */
export function fixedText(value: number, scale: number): string {
  if (!Number.isSafeInteger(value)) throw new Error('Число вне допустимого диапазона')
  const neg = value < 0
  const digits = String(Math.abs(value))
  if (scale <= 0) return `${neg ? '-' : ''}${digits}`
  const padded = digits.padStart(scale + 1, '0')
  const whole = padded.slice(0, padded.length - scale)
  const frac = padded.slice(padded.length - scale)
  return `${neg ? '-' : ''}${whole}.${frac}`
}

/** Номер дня Excel (1900, с поправкой на 29.02.1900): 2026-10-31 → 46326. */
export function excelDay(iso: string): number {
  const m = /^(\d{4})-(\d{2})-(\d{2})/.exec(iso)
  if (!m) throw new Error(`Неверная дата: ${iso}`)
  const ms = Date.UTC(Number(m[1]), Number(m[2]) - 1, Number(m[3]))
  return ms / 86_400_000 + 25_569
}

// ---------- XML ----------

/** Текст для XML: спецзнаки экранируются, недопустимые управляющие знаки убираются. */
export function xmlText(s: string): string {
  return (
    s
      // eslint-disable-next-line no-control-regex
      .replace(/[\u0000-\u0008\u000b\u000c\u000e-\u001f￾￿]/g, '')
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;')
  )
}

/** Буквы столбца: 0 → «A», 26 → «AA». */
export function columnName(i: number): string {
  let n = i + 1
  let s = ''
  while (n > 0) {
    const r = (n - 1) % 26
    s = String.fromCharCode(65 + r) + s
    n = Math.floor((n - 1) / 26)
  }
  return s
}

/** Имя листа по правилам Excel: не длиннее 31 знака, без []:*?/\ и уникальное в книге. */
export function sheetNames(names: string[]): string[] {
  const used = new Set<string>()
  return names.map((raw) => {
    const base =
      raw
        .replace(/[[\]:*?/\\]/g, ' ')
        .replace(/^'+|'+$/g, '')
        .replace(/\s+/g, ' ')
        .trim()
        .slice(0, 31)
        .trim() || 'Лист'
    let name = base
    for (let n = 2; used.has(name.toLowerCase()); n++) {
      const tail = ` (${n})`
      name = base.slice(0, 31 - tail.length).trim() + tail
    }
    used.add(name.toLowerCase())
    return name
  })
}

// Стили ячеек: вид × жирный × отступ. Номер стиля = 1 + ((вид × 2 + жирный) × ОТСТУПОВ + отступ).
const KINDS = ['text', 'money', 'int', 'qty1', 'qty2', 'qty3', 'date'] as const
type Kind = (typeof KINDS)[number]
const INDENTS = 6
// Встроенные форматы Excel: 4 — «# ##0,00», 3 — «# ##0», 14 — дата; свои — с 164.
const NUM_FMT: Record<Kind, number> = { text: 0, money: 4, int: 3, qty1: 164, qty2: 4, qty3: 165, date: 14 }
const HEADER_STYLE = 1 + KINDS.length * 2 * INDENTS

function styleId(kind: Kind, bold: boolean, indent: number): number {
  const k = KINDS.indexOf(kind)
  return 1 + (k * 2 + (bold ? 1 : 0)) * INDENTS + Math.min(Math.max(indent, 0), INDENTS - 1)
}

function stylesXml(): string {
  const xfs: string[] = ['<xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0"/>']
  for (const kind of KINDS) {
    for (const bold of [false, true]) {
      for (let indent = 0; indent < INDENTS; indent++) {
        const fmt = NUM_FMT[kind]
        const align = indent > 0 ? `<alignment horizontal="left" indent="${indent}"/>` : ''
        xfs.push(
          `<xf numFmtId="${fmt}" fontId="${bold ? 1 : 0}" fillId="0" borderId="0" xfId="0"${fmt ? ' applyNumberFormat="1"' : ''}${bold ? ' applyFont="1"' : ''}${align ? ` applyAlignment="1">${align}</xf>` : '/>'}`,
        )
      }
    }
  }
  // Шапка таблицы: жирная, серая заливка, рамка снизу, перенос слов.
  xfs.push(
    '<xf numFmtId="0" fontId="1" fillId="2" borderId="1" xfId="0" applyFont="1" applyFill="1" applyBorder="1" applyAlignment="1"><alignment vertical="center" wrapText="1"/></xf>',
  )
  return (
    '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>' +
    '<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">' +
    '<numFmts count="2"><numFmt numFmtId="164" formatCode="#,##0.0"/><numFmt numFmtId="165" formatCode="#,##0.000"/></numFmts>' +
    '<fonts count="2"><font><sz val="11"/><name val="Calibri"/></font><font><b/><sz val="11"/><name val="Calibri"/></font></fonts>' +
    '<fills count="3"><fill><patternFill patternType="none"/></fill><fill><patternFill patternType="gray125"/></fill>' +
    '<fill><patternFill patternType="solid"><fgColor rgb="FFE2E8F0"/><bgColor indexed="64"/></patternFill></fill></fills>' +
    '<borders count="2"><border><left/><right/><top/><bottom/><diagonal/></border>' +
    '<border><left/><right/><top/><bottom style="thin"><color rgb="FF94A3B8"/></bottom><diagonal/></border></borders>' +
    '<cellStyleXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellStyleXfs>' +
    `<cellXfs count="${xfs.length}">${xfs.join('')}</cellXfs>` +
    '<cellStyles count="1"><cellStyle name="Normal" xfId="0" builtinId="0"/></cellStyles>' +
    '</styleSheet>'
  )
}

/** Ячейка → XML и длина текста для ширины столбца. */
function cellXml(ref: string, cell: Cell, bold: boolean, indent: number): { xml: string; width: number } {
  if (cell === null || cell === undefined || cell === '') {
    return bold || indent ? { xml: `<c r="${ref}" s="${styleId('text', bold, indent)}"/>`, width: 0 } : { xml: '', width: 0 }
  }
  if (typeof cell === 'string') {
    const preserve = /^\s|\s$|\n/.test(cell) ? ' xml:space="preserve"' : ''
    const longest = Math.max(...cell.split('\n').map((l) => l.length))
    return {
      xml: `<c r="${ref}" t="inlineStr" s="${styleId('text', bold, indent)}"><is><t${preserve}>${xmlText(cell)}</t></is></c>`,
      width: longest + indent * 2,
    }
  }
  let kind: Kind
  let v: string
  if ('money' in cell) {
    kind = 'money'
    v = fixedText(cell.money, 2)
  } else if ('qty' in cell) {
    const scale = Math.min(Math.max(cell.scale ?? 0, 0), 3)
    kind = scale === 0 ? 'int' : scale === 1 ? 'qty1' : scale === 2 ? 'qty2' : 'qty3'
    v = fixedText(cell.qty, scale)
  } else if ('int' in cell) {
    kind = 'int'
    v = fixedText(cell.int, 0)
  } else {
    kind = 'date'
    v = String(excelDay(cell.date))
  }
  // Ширина: цифры с разделителями групп и знаком.
  const width = kind === 'date' ? 10 : v.length + Math.floor(v.replace(/^-/, '').split('.')[0].length / 3) + 1
  return { xml: `<c r="${ref}" s="${styleId(kind, bold, 0)}"><v>${v}</v></c>`, width }
}

function sheetXml(sheet: Sheet, index: number): { xml: string; filterRef: string | null } {
  const cols = Math.max(1, sheet.columns.length, ...sheet.rows.map((r) => (Array.isArray(r) ? r : r.cells).length))
  const widths = Array.from({ length: cols }, (_, i) => sheet.columns[i]?.label.length ?? 0)
  const rows: string[] = []
  let r = 0
  let maxLevel = 0
  const textRow = (text: string, bold: boolean) => {
    r++
    const c = cellXml(`A${r}`, text, bold, 0)
    rows.push(`<row r="${r}">${c.xml}</row>`)
  }
  for (const t of sheet.title ?? []) textRow(t, r === 0)
  if (sheet.title?.length) r++ // пустая строка между шапкой листа и таблицей
  r++
  const headerRow = r
  rows.push(
    `<row r="${r}">${sheet.columns
      .map((c, i) => `<c r="${columnName(i)}${r}" t="inlineStr" s="${HEADER_STYLE}"><is><t>${xmlText(c.label)}</t></is></c>`)
      .join('')}</row>`,
  )
  for (const raw of sheet.rows) {
    const row: SheetRow = Array.isArray(raw) ? { cells: raw } : raw
    const level = Math.min(Math.max(row.level ?? 0, 0), 7)
    maxLevel = Math.max(maxLevel, level)
    r++
    const cells = row.cells.map((cell, i) => {
      const c = cellXml(`${columnName(i)}${r}`, cell, row.bold ?? false, i === 0 ? level : 0)
      widths[i] = Math.max(widths[i] ?? 0, c.width)
      return c.xml
    })
    rows.push(`<row r="${r}"${level ? ` outlineLevel="${level}"` : ''}>${cells.join('')}</row>`)
  }
  const lastRow = r
  const footer = [...(sheet.footer ?? []), ...(sheet.warning ? [sheet.warning] : [])]
  if (footer.length) r++
  for (const f of footer) textRow(f, false)
  const lastCol = columnName(cols - 1)
  const filterRef = sheet.filter !== false && sheet.columns.length > 0 && lastRow > headerRow ? `A${headerRow}:${lastCol}${lastRow}` : null
  const colsXml = widths
    .map((w, i) => {
      const width = sheet.columns[i]?.width ?? Math.min(Math.max(w + 2, 8), 60)
      return `<col min="${i + 1}" max="${i + 1}" width="${width}" customWidth="1"/>`
    })
    .join('')
  const xml =
    '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>' +
    '<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">' +
    // Итог группы — над её строками (дерево отчёта раскрывается вниз).
    '<sheetPr><outlinePr summaryBelow="0"/></sheetPr>' +
    `<dimension ref="A1:${lastCol}${Math.max(r, 1)}"/>` +
    `<sheetViews><sheetView workbookViewId="0"${index === 0 ? ' tabSelected="1"' : ''}>` +
    `<pane ySplit="${headerRow}" topLeftCell="A${headerRow + 1}" activePane="bottomLeft" state="frozen"/>` +
    `<selection pane="bottomLeft" activeCell="A${headerRow + 1}" sqref="A${headerRow + 1}"/></sheetView></sheetViews>` +
    `<sheetFormatPr defaultRowHeight="15"${maxLevel ? ` outlineLevelRow="${maxLevel}"` : ''}/>` +
    `<cols>${colsXml}</cols>` +
    `<sheetData>${rows.join('')}</sheetData>` +
    (filterRef ? `<autoFilter ref="${filterRef}"/>` : '') +
    '<pageMargins left="0.5" right="0.5" top="0.6" bottom="0.6" header="0.3" footer="0.3"/>' +
    '<pageSetup paperSize="9" orientation="portrait" fitToWidth="1" fitToHeight="0"/>' +
    '</worksheet>'
  return { xml, filterRef }
}

// ---------- Zip без сжатия ----------

const CRC_TABLE = (() => {
  const t = new Uint32Array(256)
  for (let n = 0; n < 256; n++) {
    let c = n
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1
    t[n] = c >>> 0
  }
  return t
})()

/** CRC32 (как в zip): «123456789» → 0xCBF43926. */
export function crc32(data: Uint8Array): number {
  let c = 0xffffffff
  for (let i = 0; i < data.length; i++) c = CRC_TABLE[(c ^ data[i]) & 0xff] ^ (c >>> 8)
  return (c ^ 0xffffffff) >>> 0
}

/** Zip-архив без сжатия (method 0): имя → содержимое. */
export function zipStore(files: { name: string; data: Uint8Array }[]): Uint8Array {
  const enc = new TextEncoder()
  const parts: Uint8Array[] = []
  const central: Uint8Array[] = []
  let offset = 0
  // Дата файлов — 01.01.2026 00:00 (DOS-формат): содержимое от времени выгрузки не зависит.
  const dosDate = ((2026 - 1980) << 9) | (1 << 5) | 1
  for (const f of files) {
    const name = enc.encode(f.name)
    const crc = crc32(f.data)
    const local = new Uint8Array(30 + name.length)
    const lv = new DataView(local.buffer)
    lv.setUint32(0, 0x04034b50, true)
    lv.setUint16(4, 20, true) // версия для распаковки
    lv.setUint16(6, 0x0800, true) // имена в UTF-8
    lv.setUint16(8, 0, true) // без сжатия
    lv.setUint16(10, 0, true)
    lv.setUint16(12, dosDate, true)
    lv.setUint32(14, crc, true)
    lv.setUint32(18, f.data.length, true)
    lv.setUint32(22, f.data.length, true)
    lv.setUint16(26, name.length, true)
    lv.setUint16(28, 0, true)
    local.set(name, 30)
    const head = new Uint8Array(46 + name.length)
    const cv = new DataView(head.buffer)
    cv.setUint32(0, 0x02014b50, true)
    cv.setUint16(4, 20, true)
    cv.setUint16(6, 20, true)
    cv.setUint16(8, 0x0800, true)
    cv.setUint16(10, 0, true)
    cv.setUint16(12, 0, true)
    cv.setUint16(14, dosDate, true)
    cv.setUint32(16, crc, true)
    cv.setUint32(20, f.data.length, true)
    cv.setUint32(24, f.data.length, true)
    cv.setUint16(28, name.length, true)
    cv.setUint32(42, offset, true)
    head.set(name, 46)
    parts.push(local, f.data)
    central.push(head)
    offset += local.length + f.data.length
  }
  const centralSize = central.reduce((a, c) => a + c.length, 0)
  const end = new Uint8Array(22)
  const ev = new DataView(end.buffer)
  ev.setUint32(0, 0x06054b50, true)
  ev.setUint16(8, files.length, true)
  ev.setUint16(10, files.length, true)
  ev.setUint32(12, centralSize, true)
  ev.setUint32(16, offset, true)
  const all = [...parts, ...central, end]
  const out = new Uint8Array(all.reduce((a, p) => a + p.length, 0))
  let p = 0
  for (const part of all) {
    out.set(part, p)
    p += part.length
  }
  return out
}

// ---------- Книга ----------

/** Книга .xlsx из листов — байты файла. */
export function xlsxBytes(sheets: Sheet[]): Uint8Array {
  if (sheets.length === 0) throw new Error('Нет листов для выгрузки')
  const enc = new TextEncoder()
  const names = sheetNames(sheets.map((s) => s.name))
  const built = sheets.map((s, i) => sheetXml(s, i))
  const head = '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
  const defined = built
    .map((b, i) => {
      if (!b.filterRef) return ''
      const [from, to] = b.filterRef.split(':')
      const abs = (ref: string) => ref.replace(/^([A-Z]+)(\d+)$/, '$$$1$$$2')
      return `<definedName name="_xlnm._FilterDatabase" localSheetId="${i}" hidden="1">'${xmlText(names[i].replace(/'/g, "''"))}'!${abs(from)}:${abs(to)}</definedName>`
    })
    .join('')
  const files: { name: string; text: string }[] = [
    {
      name: '[Content_Types].xml',
      text:
        head +
        '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">' +
        '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>' +
        '<Default Extension="xml" ContentType="application/xml"/>' +
        '<Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>' +
        '<Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/>' +
        sheets
          .map(
            (_, i) =>
              `<Override PartName="/xl/worksheets/sheet${i + 1}.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>`,
          )
          .join('') +
        '</Types>',
    },
    {
      name: '_rels/.rels',
      text:
        head +
        '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">' +
        '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/>' +
        '</Relationships>',
    },
    {
      name: 'xl/workbook.xml',
      text:
        head +
        '<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">' +
        '<bookViews><workbookView/></bookViews><sheets>' +
        names.map((n, i) => `<sheet name="${xmlText(n)}" sheetId="${i + 1}" r:id="rId${i + 1}"/>`).join('') +
        '</sheets>' +
        (defined ? `<definedNames>${defined}</definedNames>` : '') +
        '</workbook>',
    },
    {
      name: 'xl/_rels/workbook.xml.rels',
      text:
        head +
        '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">' +
        sheets
          .map(
            (_, i) =>
              `<Relationship Id="rId${i + 1}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet${i + 1}.xml"/>`,
          )
          .join('') +
        `<Relationship Id="rId${sheets.length + 1}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>` +
        '</Relationships>',
    },
    { name: 'xl/styles.xml', text: stylesXml() },
    ...built.map((b, i) => ({ name: `xl/worksheets/sheet${i + 1}.xml`, text: b.xml })),
  ]
  return zipStore(files.map((f) => ({ name: f.name, data: enc.encode(f.text) })))
}

export const XLSX_TYPE = 'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet'

/** Книга .xlsx как файл для скачивания. */
export function writeXlsx(sheets: Sheet[]): Blob {
  return new Blob([xlsxBytes(sheets) as BlobPart], { type: XLSX_TYPE })
}

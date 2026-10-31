import { describe, expect, it } from 'vitest'
import { cellText, columnIndex } from './xlsx'
import { columnName, crc32, excelDay, fixedText, sheetNames, xlsxBytes, xmlText, type Sheet } from './xlsxWrite'

/** Файлы zip без сжатия: читаем локальные заголовки и сверяем CRC32 (как сделает Excel). */
function unzip(bytes: Uint8Array): Map<string, string> {
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength)
  const out = new Map<string, string>()
  let p = 0
  const dec = new TextDecoder()
  while (view.getUint32(p, true) === 0x04034b50) {
    expect(view.getUint16(p + 8, true)).toBe(0)
    const crc = view.getUint32(p + 14, true)
    const size = view.getUint32(p + 18, true)
    const nameLen = view.getUint16(p + 26, true)
    const name = dec.decode(bytes.subarray(p + 30, p + 30 + nameLen))
    const data = bytes.subarray(p + 30 + nameLen, p + 30 + nameLen + size)
    expect(crc32(data)).toBe(crc)
    out.set(name, dec.decode(data))
    p += 30 + nameLen + size
  }
  // Дальше — центральный каталог с тем же числом файлов.
  expect(view.getUint32(p, true)).toBe(0x02014b50)
  const end = bytes.length - 22
  expect(view.getUint32(end, true)).toBe(0x06054b50)
  expect(view.getUint16(end + 10, true)).toBe(out.size)
  return out
}

/** Строки листа как readXlsx (xlsx.ts): текст ячеек, пустые строки пропускаются. */
function rows(xml: string): string[][] {
  const out: string[][] = []
  for (const r of xml.matchAll(/<row [^>]*>([\s\S]*?)<\/row>/g)) {
    const row: string[] = []
    for (const c of r[1].matchAll(/<c r="([A-Z]+)\d+"([^>]*?)(?:\/>|>([\s\S]*?)<\/c>)/g)) {
      const i = columnIndex(c[1])
      const body = c[3] ?? ''
      const raw = /t="inlineStr"/.test(c[2]) ? [...body.matchAll(/<t[^>]*>([\s\S]*?)<\/t>/g)].map((m) => m[1]).join('') : (/<v>(.*?)<\/v>/.exec(body)?.[1] ?? '')
      const value = raw.replace(/&lt;/g, '<').replace(/&gt;/g, '>').replace(/&quot;/g, '"').replace(/&amp;/g, '&')
      while (row.length < i) row.push('')
      row[i] = cellText(value)
    }
    if (row.some((x) => x !== '')) out.push(row)
  }
  return out
}

describe('запись .xlsx', () => {
  it('CRC32 и буквы столбцов', () => {
    expect(crc32(new TextEncoder().encode('123456789'))).toBe(0xcbf43926)
    expect(columnName(0)).toBe('A')
    expect(columnName(25)).toBe('Z')
    expect(columnName(26)).toBe('AA')
    expect(columnName(701)).toBe('ZZ')
    for (const i of [0, 5, 26, 51, 700]) expect(columnIndex(`${columnName(i)}1`)).toBe(i)
  })

  it('числа из целых строкой, без дробей', () => {
    expect(fixedText(16_395_200, 2)).toBe('163952.00')
    expect(fixedText(12_050, 2)).toBe('120.50')
    expect(fixedText(1, 2)).toBe('0.01')
    expect(fixedText(99, 2)).toBe('0.99')
    expect(fixedText(100, 2)).toBe('1.00')
    expect(fixedText(-1, 2)).toBe('-0.01')
    expect(fixedText(-12_050, 2)).toBe('-120.50')
    expect(fixedText(0, 2)).toBe('0.00')
    expect(fixedText(4500, 3)).toBe('4.500')
    expect(fixedText(Number.MAX_SAFE_INTEGER, 2)).toBe('90071992547409.91')
    expect(fixedText(7, 0)).toBe('7')
    expect(() => fixedText(0.5, 2)).toThrow()
  })

  it('дата Excel', () => {
    expect(excelDay('1900-03-01')).toBe(61)
    expect(excelDay('2026-10-31')).toBe(46326)
    expect(excelDay('2026-10-31T10:00:00Z')).toBe(46326)
  })

  it('XML и имена листов', () => {
    expect(xmlText('A & B <c> "d"\u0001')).toBe('A &amp; B &lt;c&gt; &quot;d&quot;')
    expect(sheetNames(['Прибыль и убытки за октябрь 2026 года полностью', 'Расходы/статьи', 'Расходы статьи', '', "'x'"])).toEqual([
      'Прибыль и убытки за октябрь 202',
      'Расходы статьи',
      'Расходы статьи (2)',
      'Лист',
      'x',
    ])
  })

  it('книга: листы, текст, числа, шапка закреплена', () => {
    const sheets: Sheet[] = [
      {
        name: 'Товары',
        title: ['Товары', 'Выгружено 31.10.2026'],
        columns: [{ label: 'Код' }, { label: 'Название' }, { label: 'Цена' }, { label: 'Остаток, л' }, { label: 'Дата' }],
        rows: [
          ['0888602305', 'Фильтр & масло <ОРИГИНАЛ>', { money: 12_050 }, { qty: 4500, scale: 3 }, { date: '2026-10-31' }],
          ['4607012345678', 'Строка\nвторая', { money: -1 }, { int: 3 }, null],
          { cells: ['Итого', null, { money: 12_049 }], bold: true },
        ],
        footer: ['Отбор: всё'],
      },
      { name: 'Дерево', columns: [{ label: 'Строка' }, { label: 'Сумма' }], rows: [{ cells: ['Выручка', { money: 100 }] }, { cells: ['Товары', { money: 100 }], level: 1 }], filter: false },
    ]
    const files = unzip(xlsxBytes(sheets))
    expect([...files.keys()]).toEqual([
      '[Content_Types].xml',
      '_rels/.rels',
      'xl/workbook.xml',
      'xl/_rels/workbook.xml.rels',
      'xl/styles.xml',
      'xl/worksheets/sheet1.xml',
      'xl/worksheets/sheet2.xml',
    ])
    expect(files.get('xl/workbook.xml')).toContain('<sheet name="Товары" sheetId="1" r:id="rId1"/>')
    expect(files.get('xl/workbook.xml')).toContain("_xlnm._FilterDatabase")
    const s1 = files.get('xl/worksheets/sheet1.xml') ?? ''
    expect(rows(s1)).toEqual([
      ['Товары'],
      ['Выгружено 31.10.2026'],
      ['Код', 'Название', 'Цена', 'Остаток, л', 'Дата'],
      ['0888602305', 'Фильтр & масло <ОРИГИНАЛ>', '120.50', '4.500', '46326'],
      // Перевод строки в ячейке readXlsx показывает пробелом.
      ['4607012345678', 'Строка вторая', '-0.01', '3'],
      ['Итого', '', '120.49'],
      ['Отбор: всё'],
    ])
    // Коды — текстом: нули впереди и 13 знаков не становятся числом.
    expect(s1).toContain('<c r="A5" t="inlineStr"')
    expect(s1).toContain('<c r="A6" t="inlineStr"')
    // Шапка (4-я строка) закреплена, автофильтр по таблице.
    expect(s1).toContain('<pane ySplit="4" topLeftCell="A5" activePane="bottomLeft" state="frozen"/>')
    expect(s1).toContain('<autoFilter ref="A4:E7"/>')
    expect(s1).toMatch(/<col min="2" max="2" width="\d+" customWidth="1"\/>/)
    const s2 = files.get('xl/worksheets/sheet2.xml') ?? ''
    expect(s2).toContain('<row r="3" outlineLevel="1">')
    expect(s2).toContain('outlineLevelRow="1"')
    expect(s2).not.toContain('autoFilter')
    expect(rows(s2)).toEqual([
      ['Строка', 'Сумма'],
      ['Выручка', '1.00'],
      ['Товары', '1.00'],
    ])
  })

  it('без листов — ошибка', () => {
    expect(() => xlsxBytes([])).toThrow()
  })
})

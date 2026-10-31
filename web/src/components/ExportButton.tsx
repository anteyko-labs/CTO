import { useState } from 'react'
import { errorText } from '../lib/hooks'
import { downloadXlsx, fileName } from '../lib/exportTable'
import type { Sheet } from '../lib/xlsxWrite'
import { Button, toast } from './ui'

/**
 * Кнопка «В Excel» (задача 003 Н13): собирает .xlsx из листов `sheets` и скачивает его.
 * `sheets` может загружать все страницы списка (`fetchAllPages`); пока файл готовится, кнопка
 * пишет «Готовлю файл…» и второй раз не срабатывает. Ошибка (нет связи) — уведомлением.
 */
export function ExportButton({
  file,
  period,
  sheets,
  label = 'В Excel',
  disabled = false,
  className = '',
}: {
  /** Название файла без расширения: «Расходы». */
  file: string
  /** Период или дата в имени файла: «01.10.2026–31.10.2026». */
  period?: string
  sheets: () => Sheet[] | Promise<Sheet[]>
  label?: string
  disabled?: boolean
  className?: string
}) {
  const [busy, setBusy] = useState(false)
  const run = async () => {
    if (busy) return
    setBusy(true)
    try {
      const list = await sheets()
      downloadXlsx(fileName(file, period), list)
      const warning = list.map((s) => s.warning).find(Boolean)
      if (warning) toast(warning, 'error')
    } catch (e) {
      toast(errorText(e), 'error')
    } finally {
      setBusy(false)
    }
  }
  return (
    <Button variant="secondary" className={`no-print ${className}`} disabled={disabled || busy} data-testid="export-xlsx" onClick={() => void run()}>
      {busy ? 'Готовлю файл…' : label}
    </Button>
  )
}

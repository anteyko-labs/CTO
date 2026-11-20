// Рабочий день по лицу на терминале Dahua (задача 039 A, ADR-200): настройки терминала и табель.

export interface FaceIdEmployee {
  id: string
  full_name: string
  faceid_user_id: string | null
}

export interface FaceIdEvent {
  created_at: string
  user_id: string | null
  employee: string | null
  outcome: 'applied' | 'no_request' | 'unknown_user' | 'bad_key' | 'no_user' | 'failed_face'
}

export interface FaceIdSettings {
  key: string
  path: string
  window_min: number
  employees: FaceIdEmployee[]
  events: FaceIdEvent[]
}

export interface WorkSession {
  id: string
  started_at: string
  ended_at: string | null
  status: 'closed' | 'open' | 'unclosed'
  minutes: number | null
  closed_by: string | null
  close_reason: string | null
}

export interface TimesheetDay {
  date: string
  minutes: number
  unclosed: boolean
  sessions: WorkSession[]
}

export interface TimesheetRow {
  employee_id: string
  full_name: string
  total_minutes: number
  days: TimesheetDay[]
}

export interface Timesheet {
  month: string
  days_in_month: number
  rows: TimesheetRow[]
}

export const OUTCOME_LABEL: Record<FaceIdEvent['outcome'], string> = {
  applied: 'день начат или закончен',
  no_request: 'без нажатия кнопки — ничего',
  unknown_user: 'неизвестный номер',
  bad_key: 'чужой ключ',
  no_user: 'нет номера в событии',
  failed_face: 'лицо не узнано',
}

/** Часы и минуты: 510 → «8:30». */
export function hours(minutes: number): string {
  const h = Math.floor(minutes / 60)
  const m = minutes % 60
  return `${h}:${String(m).padStart(2, '0')}`
}

/** Время по Бишкеку (UTC+6) из ISO-строки: «09:02». */
export function bishkekTime(iso: string): string {
  return new Date(iso).toLocaleTimeString('ru-RU', { hour: '2-digit', minute: '2-digit', timeZone: 'Asia/Bishkek' })
}

/** Момент по дате и времени по Бишкеку: «2026-10-06», «18:00» → ISO. */
export function bishkekMoment(date: string, time: string): string {
  return new Date(`${date}T${time}:00+06:00`).toISOString()
}

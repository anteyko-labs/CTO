// Переключатель «включено / выключено» для строки справочника: состояние видно сразу, без отдельной кнопки.
export function DirSwitch({
  checked,
  onChange,
  label,
  disabled = false,
}: {
  checked: boolean
  onChange: (v: boolean) => void
  /** Что включаем: озвучивается и показывается при наведении. */
  label: string
  disabled?: boolean
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      title={checked ? `${label}: включено` : `${label}: выключено`}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      className="inline-flex items-center gap-2 text-sm disabled:cursor-not-allowed disabled:opacity-60"
    >
      <span className={`relative inline-flex h-6 w-11 shrink-0 rounded-full transition ${checked ? 'bg-emerald-600' : 'bg-slate-300'}`}>
        <span className={`absolute top-0.5 h-5 w-5 rounded-full bg-white shadow transition-all ${checked ? 'left-[22px]' : 'left-0.5'}`} />
      </span>
      <span className={checked ? 'text-emerald-700' : 'text-slate-500'}>{checked ? 'включено' : 'выключено'}</span>
    </button>
  )
}

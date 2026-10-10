// Маленький значок «?» с подсказкой: длинное пояснение не растягивает форму.
export function DirHint({ text }: { text: string }) {
  return (
    <span
      role="img"
      aria-label={text}
      title={text}
      tabIndex={0}
      className="ml-1 inline-flex h-4 w-4 cursor-help items-center justify-center rounded-full border border-slate-300 align-middle text-[10px] font-semibold text-slate-500"
    >
      ?
    </span>
  )
}

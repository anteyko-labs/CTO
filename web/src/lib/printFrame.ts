// Печать через окно браузера (решение заказчика — без своих драйверов): документ в невидимом
// фрейме, затем фокус возвращается туда, где был. Иначе после автопечати книжки или документа о
// долге фокус оставался во фрейме и следующий скан товара терялся (ui-rules §4).

/** Напечатать готовый HTML-документ. */
export function printInFrame(html: string): void {
  const back = document.activeElement instanceof HTMLElement ? document.activeElement : null
  const frame = document.createElement('iframe')
  Object.assign(frame.style, { position: 'fixed', width: '0', height: '0', border: '0' })
  document.body.appendChild(frame)
  const doc = frame.contentDocument
  const win = frame.contentWindow
  if (!doc || !win) {
    frame.remove()
    return
  }
  doc.open()
  doc.write(html)
  doc.close()
  const restore = () => {
    if (back && back.isConnected) back.focus()
  }
  win.addEventListener('afterprint', restore, { once: true })
  const print = () => {
    win.focus()
    win.print()
    // Диалог печати в Chromium блокирует до закрытия: сразу после — вернуть фокус.
    restore()
    setTimeout(() => frame.remove(), 60_000)
  }
  // Картинка подписи (задача 008) ещё не разобрана — печать после неё, иначе на листе пусто.
  const pending = Array.from(doc.images).filter((img) => !img.complete)
  if (pending.length === 0) {
    print()
    return
  }
  let left = pending.length
  const one = () => {
    left -= 1
    if (left === 0) print()
  }
  for (const img of pending) {
    img.addEventListener('load', one, { once: true })
    img.addEventListener('error', one, { once: true })
  }
}

/** Напечатать документ самой страницей, без фрейма: на XP-365B (драйвер Seagull) печать из
 *  фрейма выходила пустой, а та же этикетка страницей — чётко (проверка 03.11.2026).
 *  Стили и содержимое документа на время печати подкладываются в страницу, остальное скрыто. */
export function printAsPage(html: string): void {
  const parsed = new DOMParser().parseFromString(html, 'text/html')
  const style = document.createElement('style')
  style.textContent =
    Array.from(parsed.querySelectorAll('style'))
      .map((s) => s.textContent ?? '')
      .join('\n') + '\nbody > *:not([data-print-page]) { display: none !important; }'
  const root = document.createElement('div')
  root.setAttribute('data-print-page', '')
  root.innerHTML = parsed.body.innerHTML
  // Стили программы на время печати выключены: в них @page A4 для отчётов, и этикетка
  // печаталась на лист A4 — пусто и на несколько наклеек (проверка 03.11.2026).
  const sheets = Array.from(document.querySelectorAll<HTMLStyleElement | HTMLLinkElement>('style, link[rel="stylesheet"]'))
  const media = sheets.map((el) => el.media)
  sheets.forEach((el) => (el.media = 'not all'))
  document.head.appendChild(style)
  document.body.appendChild(root)
  try {
    window.print()
  } finally {
    root.remove()
    style.remove()
    sheets.forEach((el, i) => (el.media = media[i]))
  }
}

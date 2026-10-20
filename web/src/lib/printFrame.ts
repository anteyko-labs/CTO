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
  win.focus()
  win.print()
  // Диалог печати в Chromium блокирует до закрытия: сразу после — вернуть фокус.
  restore()
  setTimeout(() => frame.remove(), 60_000)
}

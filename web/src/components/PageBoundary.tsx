// Экран, загружаемый отдельной частью сборки, не загрузился — сообщение вместо пустой страницы.
import { Component, type ReactNode } from 'react'

/** Экран не загрузился и после перезагрузки — сообщение вместо пустой страницы. */
export class PageBoundary extends Component<{ children: ReactNode; resetKey: string }, { failed: boolean; key: string }> {
  state = { failed: false, key: this.props.resetKey }

  static getDerivedStateFromError() {
    return { failed: true }
  }

  static getDerivedStateFromProps(props: { resetKey: string }, state: { failed: boolean; key: string }) {
    return props.resetKey === state.key ? null : { failed: false, key: props.resetKey }
  }

  render() {
    if (!this.state.failed) return this.props.children
    return (
      <div className="flex flex-col items-start gap-3 rounded-md border border-amber-200 bg-amber-50 p-4 text-sm text-amber-900" role="alert">
        Экран не загрузился — нет связи с сервером или программа обновилась.
        <button type="button" className="min-h-[44px] rounded-md bg-sky-600 px-4 font-medium text-white" onClick={() => window.location.reload()}>
          Обновить страницу
        </button>
      </div>
    )
  }
}

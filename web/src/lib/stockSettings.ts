// «Настройки → Склад» (`GET/PUT /settings/stock`): срок залежалости (ADR-039), норма по маслу на
// ревизии (ADR-052) и режим цены масла по умолчанию (задача 003, Н7, ADR-{B6.1}). Читают все,
// меняет владелец; без связи — как по умолчанию.
import { useEffect, useState } from 'react'
import { get } from './api'
import { DEFAULT_OIL_PRICE_MODES, type OilPriceModes, type QtyMode } from './receiptLines'

export interface StockSettings {
  stale_days: number
  oil_norm_bp: number
  oil_price_mode_receipt: QtyMode
  oil_price_mode_stock_in: QtyMode
}

export const oilPriceModes = (s: Pick<StockSettings, 'oil_price_mode_receipt' | 'oil_price_mode_stock_in'>): OilPriceModes => ({
  receipt: s.oil_price_mode_receipt === 'l' ? 'l' : 'can',
  stockIn: s.oil_price_mode_stock_in === 'can' ? 'can' : 'l',
})

/** Режим цены масла по умолчанию; пока не загрузился или нет связи — как было (кан. / л). */
export function useOilPriceModes(): { modes: OilPriceModes; loaded: boolean } {
  const [modes, setModes] = useState<{ modes: OilPriceModes; loaded: boolean }>({ modes: DEFAULT_OIL_PRICE_MODES, loaded: false })
  useEffect(() => {
    let alive = true
    get<StockSettings>('/settings/stock')
      .then((s) => alive && setModes({ modes: oilPriceModes(s), loaded: true }))
      .catch(() => alive && setModes({ modes: DEFAULT_OIL_PRICE_MODES, loaded: true }))
    return () => {
      alive = false
    }
  }, [])
  return modes
}

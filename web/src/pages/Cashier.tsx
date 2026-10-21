// Экран «Касса»: чек на вынос или в сервис, масло канистрой и на розлив, оплата, клиент и долг,
// подарки, работа без сети (SPEC-04, SPEC-09, SPEC-10, SPEC-11).
import { useEffect, useRef, useState } from 'react'
import { Link, useNavigate } from 'react-router-dom'
import { CashierFavorites } from '../components/CashierFavorites'
import { CashierModeBar } from '../components/CashierModeBar'
import { ShiftOpenModal } from '../components/ShiftOpen'
import { CorrectionBanner } from '../components/Correction'
import { balanceText, ClientPicker } from '../components/ClientPicker'
import { GiftPicker } from '../components/GiftPicker'
import { QuickExpense } from '../components/QuickExpense'
import { ProductFormModal } from '../components/ProductFormModal'
import { ProductPicker, stockText } from '../components/ProductPicker'
import { DebtorPayment } from '../components/RepaymentModal'
import { BatteryIntake, type BatteryInfo } from '../components/BatteryIntake'
import { BonusRedeem, type BonusUse } from '../components/BonusRedeem'
import { batteryAmount, productTitle } from '../lib/battery'
import { OilHint } from '../components/OilBook'
import { nextText, recordLines, recordMileage, slotLabel, slotNextText, slotsWithNext, type VehicleBook } from '../lib/oilBook'
import { fromKm, mileageText, unitLabel, type MileageUnit } from '../lib/mileage'
import { hasEngineOil } from '../lib/oilChange'
import { CLIENT_NEED_LABELS, isDiscount, serviceClientMissing, servicePrice } from '../lib/serviceSale'
import { printOilBook } from '../lib/oilBookPrint'
import { ServicePicker } from '../components/ServicePicker'
import { UnknownCodeModal } from '../components/UnknownCodeModal'
import { printSale } from '../components/salePrint'
import { Icon, type IconName } from '../components/icons'
import { Badge, Button, Card, ErrorBox, Field, Missing, Modal, Money, RowMenu, toast } from '../components/ui'
import { ApiError, get, newOpId, noServerAnswer, patch, post, qs } from '../lib/api'
import { printDebtDoc, type DebtDocSettings, type Reconciliation } from '../lib/debtDocs'
import { enqueueSale, keepInQueue, knownShift, markPending, markSent, readSnapshot, removeSale, requestPersistentStorage, saveKnownShift, takeFromSnapshot } from '../lib/offline'
import { bankChoices, defaultBankId, isTill, payCaption, type NoncashMethod } from '../lib/accounts'
import { forServer, removedLine, sendCancelled, unremove, type CancelReason, type RemovedLine } from '../lib/cancelledLines'
import { useUser } from '../lib/auth'
import { can } from '../lib/permissions'
import { enterFullscreen, setCashierMode, useNumField, useTouchMode } from '../lib/cashierMode'
import { postCorrection } from '../lib/corrections'
import type { SaleFixState } from '../lib/saleFix'
import { formatLiters, formatSom, litersInput, parseLiters, parseSom, somInput } from '../lib/format'
import { useFocusAfterRender } from '../lib/focus'
import { missingWithFocus } from '../lib/forms'
import { useAction, useLoad, usePolling, useServerOnline } from '../lib/hooks'
import { pourAmount, pourPricePerL } from '../lib/money'
import { PAYMENT_LABELS, type CashAccount, type Employee, type GiftRule, type Party, type PaymentMethod, type Product, type Sale, type SaleLineKind, type Service, type Shift } from '../lib/types'

interface CartLine {
  key: string
  kind: SaleLineKind
  product?: Product
  /** Работа из справочника услуг: цена и ставка мастера (ADR-043). */
  service?: Service
  qtyText: string
  priceText: string
  /** Подарок: цена ноль, в чеке помечен (SPEC-11). */
  gift?: boolean
}

type PayMode = PaymentMethod | 'mixed'

const CASHIER_KEY = 'avtodom.cashier_id'
type BankChoice = Pick<CashAccount, 'id' | 'name' | 'kind' | 'default_card' | 'default_transfer' | 'default_invoice'> & { fee_rate_bp?: number }

/** Безналичные счета с отметками «по умолчанию для карты / перевода» (ADR-077): с сервера, без
 * связи — из снимка кассы. */
async function loadBanks(): Promise<BankChoice[]> {
  try {
    return bankChoices(await get<CashAccount[]>('/cash/accounts'))
  } catch {
    return ((await readSnapshot())?.bank_accounts ?? []).map((b) => ({ ...b, kind: 'bank' as const }))
  }
}

const NO_BANK_PICK: Record<NoncashMethod, string> = { card: '', transfer: '', invoice: '' }

function remembered(key: string): string {
  try {
    return localStorage.getItem(key) ?? ''
  } catch {
    return ''
  }
}

function remember(key: string, value: string): void {
  try {
    localStorage.setItem(key, value)
  } catch {
    // Хранилище недоступно — выбор просто не запомнится.
  }
}

/** Цена по каталогу; `ourOil` — в чеке есть наше масло: вид замены «бесплатно с нашим маслом»
 *  стоит 0 (ADR-096). */
function listPrice(l: Pick<CartLine, 'kind' | 'product' | 'service'>, ourOil = false): number {
  if (l.kind === 'service') return servicePrice(l.service, ourOil)
  // Розлив — от цены канистры (ADR-074): 4 л розливом стоят как канистра 4 л.
  if (l.kind === 'pour') return (l.product && pourPricePerL(l.product)) ?? 0
  return l.product?.sale_price_tyiyn ?? 0
}

/** Количество строки в единицах запроса: шт, канистры или мл для розлива. */
function lineQty(l: CartLine): number | null {
  // На вес — килограммы с граммами, как литры розлива: «12,35» → 12 350 г.
  if (l.kind === 'pour' || l.kind === 'weight') {
    const ml = parseLiters(l.qtyText)
    return ml && ml > 0 ? ml : null
  }
  return /^\d+$/.test(l.qtyText.trim()) && Number(l.qtyText) > 0 ? Number(l.qtyText) : null
}

function lineAmount(l: CartLine): number | null {
  const qty = lineQty(l)
  const price = parseSom(l.priceText)
  if (qty === null || price === null) return null
  const amount = l.kind === 'pour' || l.kind === 'weight' ? pourAmount(qty, price) : qty * price
  return Number.isSafeInteger(amount) ? amount : null
}

const nextKey = () => crypto.randomUUID()

/** Незавершённый чек переживает переход на другой экран и перезагрузку вкладки. */
const DRAFT_KEY = 'avtodom.cart'

/** Отложенные чеки: клиент ушёл за второй канистрой, а касса свободна. */
const PARKED_KEY = 'avtodom.parked'

interface Parked {
  id: string
  at: string
  label: string
  saleType: 'takeaway' | 'service'
  masterId: string
  lines: CartLine[]
  comment: string
  party: Party | null
  contactId: string
  vehicleId: string
  mileage?: string
  delivery?: string | null
  declined?: string
  /** Строки, удалённые крестиком до того, как чек отложили (SPEC-30 §8). */
  dropped?: RemovedLine[]
}

function loadParked(): Parked[] {
  try {
    const raw = localStorage.getItem(PARKED_KEY)
    return raw ? (JSON.parse(raw) as Parked[]) : []
  } catch {
    return []
  }
}

function saveParked(list: Parked[]): void {
  try {
    localStorage.setItem(PARKED_KEY, JSON.stringify(list))
  } catch {
    // Без хранилища отложенные живут до перезагрузки.
  }
}

const parkedTotal = (p: Parked): number => p.lines.reduce((acc, l) => acc + (lineAmount(l) ?? 0), 0)

interface Draft {
  saleType: 'takeaway' | 'service'
  masterId: string
  lines: CartLine[]
  comment: string
  // Ушли по ссылке и вернулись — клиент, машина и пробег не теряются.
  party?: Party | null
  contactId?: string
  vehicleId?: string
  mileage?: string
  delivery?: string | null
  declined?: string
  dropped?: RemovedLine[]
}

function loadDraft(): Draft | null {
  try {
    const raw = sessionStorage.getItem(DRAFT_KEY)
    return raw ? (JSON.parse(raw) as Draft) : null
  } catch {
    return null
  }
}

function saveDraft(d: Draft): void {
  try {
    if (d.lines.length === 0 && !d.comment) sessionStorage.removeItem(DRAFT_KEY)
    else sessionStorage.setItem(DRAFT_KEY, JSON.stringify(d))
  } catch {
    // Без хранилища черновик просто не сохранится.
  }
}

/** Единицы учёта строки (шт или мл) для сравнения с остатком. */
function lineUnits(l: CartLine): number {
  const qty = lineQty(l) ?? 0
  if (l.kind === 'container') return qty * (l.product?.container_ml ?? 0)
  return qty
}

/** Купюры для быстрого ввода полученной суммы, тыйын. */
const NOTES = [10_000, 20_000, 50_000, 100_000, 200_000, 500_000]

/** Значки способов оплаты на кнопках режима кассы. */
const PAY_ICONS: Record<PayMode, IconName> = { cash: 'wallet', card: 'bank', transfer: 'scan', invoice: 'receipt', debt: 'debts', mixed: 'split', bonus: 'star', battery: 'stock' }

/** Способы оплаты кнопками: 3 + 3 — наличные, карта, QR сверху — частые; «По счёту», долг и
 *  смешанная ниже. */
const PAY_MODES = ['cash', 'card', 'transfer', 'invoice', 'debt', 'mixed'] as const
/** Способы в смешанной оплате. */
const SPLIT_METHODS = ['cash', 'card', 'transfer', 'invoice', 'debt'] as const
const NO_SPLIT: Record<(typeof SPLIT_METHODS)[number], string> = { cash: '', card: '', transfer: '', invoice: '', debt: '' }

/** Кнопка «−» / «+» у количества: в режиме кассы — отдельный крупный квадрат (ui-rules §9). */
function qtyBtn(touch: boolean, side: 'l' | 'r'): string {
  if (touch) return 'flex min-h-[56px] min-w-[56px] items-center justify-center rounded-lg border border-slate-300 bg-slate-50 text-3xl font-semibold text-slate-800 active:bg-slate-200'
  return `min-h-12 min-w-12 border border-slate-300 px-3 text-xl text-slate-700 hover:bg-slate-50 active:bg-slate-100 ${side === 'l' ? 'rounded-l-md border-r-0' : 'rounded-r-md border-l-0'}`
}

const focusPicker = () => document.querySelector<HTMLInputElement>('[data-picker]')?.focus()

/** Исправление чека владельцем (SPEC-29 §24): тот же экран кассы, заполненный исходным чеком. */
export interface CashierFix {
  originalId: string
  number: number | null
  state: SaleFixState
}

export default function Cashier({ fix }: { fix?: CashierFix }) {
  const navigate = useNavigate()
  const focusAfterRender = useFocusAfterRender()
  const user = useUser()
  const owner = user.role === 'owner'
  // Права администратора (ADR-097): без права кнопка или способ оплаты скрыты, проверяет сервер.
  const payAllowed = (m: string) => m !== 'debt' || can(user, 'sale_debt')
  // Скидка — цена ниже каталожной: владелец или администратор с правом «Скидка в чеке» (ADR-097).
  // Без права касса не даёт править цену строки; проверяет сервер.
  const canDiscount = can(user, 'discount')
  // Связь с сервером, а не только сеть устройства: баллы и аккумулятор считает сервер.
  const online = useServerOnline()
  // Режим кассы для сенсорного моноблока (SPEC-29): без меню, всё крупнее, цифры — экранной
  // клавиатурой. В исправлении чека владельцем режима нет.
  const touch = useTouchMode() && !fix
  const num = useNumField()
  const [shiftOpen, setShiftOpen] = useState<{ till: CashAccount | undefined } | null>(null)
  // Без связи — сотрудники из снимка кассы: кассир и мастер выбираются и после перезагрузки (SPEC-09).
  const employees = useLoad(
    () =>
      get<Employee[]>('/employees').catch(async (e: unknown) => {
        const snap = noServerAnswer(e) ? await readSnapshot() : null
        if (!snap) throw e
        return snap.employees.map((x) => ({ ...x, active: true }))
      }),
    [],
  )
  // Куда идут карта и QR: у каждого способа свой счёт по умолчанию, кассир может выбрать другой
  // банк для этого чека (ADR-076, ADR-077).
  const banks = useLoad(loadBanks, [])
  const [bankPick, setBankPick] = useState<Record<NoncashMethod, string>>(fix?.state.bankPick ?? NO_BANK_PICK)
  const bankList = banks.data ?? []
  const bankFor = (method: NoncashMethod): string =>
    bankList.some((b) => b.id === bankPick[method]) ? bankPick[method] : defaultBankId(bankList, method)
  // Плашка смены: продавать можно и без неё, но деньги потом не с чем сверить (SPEC-05).
  // Без связи остаётся последняя известная смена: касса не начнёт спрашивать кассира посреди смены.
  // Она переживает и перезагрузку страницы: хранится на устройстве (SPEC-09).
  const lastShift = useRef<Shift | null | undefined>(fix ? undefined : knownShift())
  const shift = useLoad(
    () =>
      get<Shift | null>('/shifts/current')
        .then((s) => {
          if (!fix) saveKnownShift(s)
          return (lastShift.current = s)
        })
        .catch(() => lastShift.current),
    [],
  )
  // Смену могли открыть или закрыть на другом устройстве: кассир чека меняется вместе с ней.
  usePolling(shift.reload, 30_000)
  // Исправление начинает с исходного чека и не трогает незавершённый чек кассы.
  const [draft] = useState<Draft | null>(() =>
    fix
      ? {
          saleType: fix.state.saleType,
          masterId: fix.state.masterId,
          lines: fix.state.lines,
          comment: fix.state.comment,
          party: fix.state.party,
          contactId: fix.state.contactId,
          vehicleId: fix.state.vehicleId,
          mileage: fix.state.mileage,
          delivery: fix.state.delivery,
          declined: fix.state.declined,
        }
      : loadDraft(),
  )
  // Учётный день исправленного чека и «что и почему исправляете» (SPEC-29 §24).
  const [businessDate, setBusinessDate] = useState(fix?.state.businessDate ?? '')
  const [fixComment, setFixComment] = useState('')
  const [saleType, setSaleType] = useState<'takeaway' | 'service'>(draft?.saleType ?? 'takeaway')
  const [cashierId, setCashierId] = useState(() => fix?.state.cashierId ?? remembered(CASHIER_KEY))
  const [masterId, setMasterId] = useState(draft?.masterId ?? '')
  const [lines, setLines] = useState<CartLine[]>(draft?.lines ?? [])
  // Строки, удалённые крестиком: уйдут с чеком в «Отменённые товары» (SPEC-30 §8).
  const [dropped, setDropped] = useState<RemovedLine[]>(draft?.dropped ?? [])
  const [comment, setComment] = useState(draft?.comment ?? '')
  const [payMode, setPayMode] = useState<PayMode>(fix?.state.payMode ?? 'cash')
  const [received, setReceived] = useState('')
  const [split, setSplit] = useState<Record<(typeof SPLIT_METHODS)[number], string>>(fix?.state.split ?? NO_SPLIT)
  // Скидка баллами: часть чека оплачивается баллами клиента из бота (SPEC-19).
  const [bonus, setBonus] = useState<BonusUse | null>(() =>
    fix && fix.state.bonusAmount > 0 && fix.state.party
      ? { party_id: fix.state.party.id, name: fix.state.party.name, balance_tyiyn: fix.state.bonusAmount, amount_tyiyn: fix.state.bonusAmount }
      : null,
  )
  const [bonusOpen, setBonusOpen] = useState(false)
  // Старый аккумулятор в оплату: вес × цена приёма за кг; сумму считает сервер (SPEC-27).
  const [batteryPay, setBatteryPay] = useState<{ kgText: string; perKgText: string } | null>(fix?.state.battery ?? null)
  const batteryLoad = useAction()
  const [confirmClear, setConfirmClear] = useState(false)
  // op_id попытки: повтор того же чека идёт с прежним, любое изменение чека даёт новый.
  const attempt = useRef<{ key: string; opId: string } | null>(null)
  const [done, setDone] = useState<{ sale: Sale; change: number | null; book?: VehicleBook | null } | null>(null)
  const [unknownCode, setUnknownCode] = useState<string | null>(null)
  const [newProductCode, setNewProductCode] = useState<string | null>(null)
  const [newProductName, setNewProductName] = useState<string | null>(null)
  const [party, setParty] = useState<Party | null>(draft?.party ?? null)
  const [parked, setParked] = useState<Parked[]>(loadParked)
  const [parkedId, setParkedId] = useState<string | null>(null)
  const [giftRule, setGiftRule] = useState<GiftRule | null>(null)
  // Правила подарков по товарам чека и те, от которых в этом чеке уже отказались.
  const [giftRules, setGiftRules] = useState<Record<string, GiftRule>>({})
  // Сколько товара-условия было в чеке, когда о подарке уже спросили: отказ не навсегда,
  // докупили больше — касса спросит снова.
  const [giftAsked, setGiftAsked] = useState<Record<string, number>>({})
  const [expense, setExpense] = useState(false)
  const [servicesOpen, setServicesOpen] = useState(false)
  // «В сервис» сразу открывает выбор вида замены (ADR-096).
  const [kindsOpen, setKindsOpen] = useState(false)
  const clientFix = useAction()
  const [clientForm, setClientForm] = useState<{ first: string; last: string; phone: string }>({ first: '', last: '', phone: '' })
  const [debtPay, setDebtPay] = useState(false)
  const [batteryIntake, setBatteryIntake] = useState(false)
  // Средняя закупка аккумуляторов за кг: подсказка «дешевле не продать» (ADR-048).
  const [batteryAvg, setBatteryAvg] = useState<number | null>(null)
  const limitAsk = useAction()
  const innSave = useAction()
  const [innText, setInnText] = useState('')
  const [contactId, setContactId] = useState(draft?.contactId ?? '')
  const [vehicleId, setVehicleId] = useState(draft?.vehicleId ?? '')
  const [mileage, setMileage] = useState(draft?.mileage ?? '')
  const [delivery, setDelivery] = useState<string | null>(draft?.delivery ?? null)
  // Что мастер советовал, а клиент отказался: попадёт в масляную книжку и подсказку в следующий раз.
  const [declined, setDeclined] = useState(draft?.declined ?? '')
  // «Это замена масла?» (SPEC-29 §18): для этого чека уже спросили — больше не спрашиваем.
  const [oilAsked, setOilAsked] = useState(false)
  const [oilQuestion, setOilQuestion] = useState(false)
  const [commentOpen, setCommentOpen] = useState(false)
  // Строка, у которой сейчас правят цену: в остальных цена видна текстом.
  const [priceEdit, setPriceEdit] = useState<string | null>(null)
  // Режим кассы: клиент и машина выбираются в окне, в колонке — короткая строка.
  const [clientOpen, setClientOpen] = useState(false)
  // Удалённая строка: без вопроса «точно?», но её можно вернуть одним касанием.
  const [removed, setRemoved] = useState<{ line: CartLine; index: number } | null>(null)
  useEffect(() => {
    if (!removed) return
    const t = setTimeout(() => setRemoved(null), 8000)
    return () => clearTimeout(t)
  }, [removed])
  // Список чека прокручивается сам по себе: новая строка (она внизу) всегда на виду.
  const cartRef = useRef<HTMLDivElement>(null)
  const lineCount = useRef(lines.length)
  useEffect(() => {
    const el = cartRef.current
    if (el && lines.length > lineCount.current) el.scrollTop = el.scrollHeight
    lineCount.current = lines.length
  }, [lines.length])
  // Аналоги фильтров по кросс-номерам: подсказка в строке, особенно когда товара нет (SPEC-17).
  const [analogs, setAnalogs] = useState<Record<string, { id: string; name: string; stock_qty: number; sale_price_tyiyn: number }[]>>({})
  const { busy, error, setError, run } = useAction()

  // Исправление оставляет кассира и мастера исходного чека, даже если их уже отключили.
  const cashiers = employees.data?.filter((e) => e.is_cashier && (e.active || e.id === fix?.state.cashierId)) ?? []
  const masters = employees.data?.filter((e) => e.is_master && (e.active || e.id === fix?.state.masterId)) ?? []

  useEffect(() => {
    if (!fix) saveDraft({ saleType, masterId, lines, comment, party, contactId, vehicleId, mileage, delivery, declined, dropped })
  }, [fix, saleType, masterId, lines, comment, party, contactId, vehicleId, mileage, delivery, declined, dropped])

  // Браузер отказался хранить данные надёжно — неотправленные чеки могут пропасть (ADR-062).
  const [storageAtRisk, setStorageAtRisk] = useState(false)
  useEffect(() => {
    let alive = true
    void requestPersistentStorage().then((ok) => {
      if (alive) setStorageAtRisk(ok === false)
    })
    return () => {
      alive = false
    }
  }, [])

  // Открыта смена — кассир чека тот, кто открыл смену, касса его не спрашивает (ADR-071); сервер
  // сам ставит кассира смены. Он же запоминается на устройстве: если связь пропадёт, чек без сети
  // уйдёт с ним. Без смены кассира выбирают один раз, выбор помнится до открытия смены.
  // Исправление: кассир — исходного чека (владелец может выбрать другого), не смены.
  const shiftCashier = fix ? null : (shift.data?.cashier_employee_id ?? null)
  // Выбор кассира показывается, только когда уже известно, что смены нет: иначе он мелькнёт
  // на время загрузки и исчезнет.
  const [shiftKnown, setShiftKnown] = useState(false)
  useEffect(() => {
    if (!shift.loading) setShiftKnown(true)
  }, [shift.loading])
  useEffect(() => {
    if (!shiftCashier) return
    setCashierId(shiftCashier)
    remember(CASHIER_KEY, shiftCashier)
  }, [shiftCashier])

  // Единственный кассир или мастер выбирается сам; выбор несуществующего сбрасывается.
  useEffect(() => {
    if (!employees.data) return
    const ids = cashiers.map((e) => e.id)
    // По текущему значению, а не по замкнутому: кассира могли выбрать, пока список грузился.
    setCashierId((cur) => (cur === shiftCashier || ids.includes(cur) ? cur : ids.length === 1 ? ids[0] : ''))
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [employees.data])
  useEffect(() => {
    if (saleType === 'service' && masters.length === 1) setMasterId((cur) => cur || masters[0].id)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [saleType, employees.data])

  // Сколько единиц каждого товара уже в чеке — для предупреждения о нехватке.
  const unitsInCart = new Map<string, number>()
  for (const l of lines) {
    if (l.product) unitsInCart.set(l.product.id, (unitsInCart.get(l.product.id) ?? 0) + lineUnits(l))
  }

  // Наше масло в чеке: вид замены «бесплатно с нашим маслом» стоит 0 (ADR-096).
  const ourOil = hasEngineOil(lines)
  const lineList = (l: Pick<CartLine, 'kind' | 'product' | 'service'>): number => listPrice(l, ourOil)
  // Масло добавили или убрали — цена такого вида замены пересчитывается, если её не меняли руками.
  useEffect(() => {
    setLines((ls) => {
      let changed = false
      const next = ls.map((l) => {
        if (l.kind !== 'service' || !l.service?.free_with_oil) return l
        if (parseSom(l.priceText) !== listPrice(l, !ourOil)) return l
        changed = true
        return { ...l, priceText: somInput(listPrice(l, ourOil)) }
      })
      return changed ? next : ls
    })
  }, [ourOil])

  const amounts = lines.map(lineAmount)
  const valid = lines.length > 0 && amounts.every((a) => a !== null)
  const total = amounts.reduce<number>((acc, a) => acc + (a ?? 0), 0)
  const bonusAmount = bonus?.amount_tyiyn ?? 0
  const batteryGrams = batteryPay ? parseLiters(batteryPay.kgText) : null
  // Цену за кг кассир может поменять в чеке: по умолчанию — цена приёма из настроек (ADR-078).
  const batteryPerKg = batteryPay ? parseSom(batteryPay.perKgText) : null
  const batteryAmt = batteryPay && batteryGrams && batteryGrams > 0 && batteryPerKg && batteryPerKg > 0 ? batteryAmount(batteryGrams, batteryPerKg) : 0
  // К оплате деньгами или в долг — итог за вычетом баллов и сданного аккумулятора.
  const due = total - bonusAmount - batteryAmt

  // Карта, QR и «По счёту» — с выбранным банком; без выбора сервер возьмёт счёт по умолчанию для способа.
  const withBank = (method: PaymentMethod, amount_tyiyn: number): { method: PaymentMethod; amount_tyiyn: number; account_id?: string } =>
    (method === 'card' || method === 'transfer' || method === 'invoice') && bankFor(method) ? { method, amount_tyiyn, account_id: bankFor(method) } : { method, amount_tyiyn }
  const moneyPayments = ((): { method: PaymentMethod; amount_tyiyn: number; account_id?: string }[] | null => {
    if (payMode !== 'mixed') return due > 0 ? [withBank(payMode, due)] : []
    const list: { method: PaymentMethod; amount_tyiyn: number; account_id?: string }[] = []
    for (const m of SPLIT_METHODS) {
      if (!split[m].trim()) continue
      const v = parseSom(split[m])
      if (v === null) return null
      if (v > 0) list.push(withBank(m, v))
    }
    return list
  })()
  // Способы безнала в чеке: для каждого — строка выбора банка.
  const noncashMethods = (['card', 'transfer', 'invoice'] as const).filter((m) => (moneyPayments ?? []).some((p) => p.method === m))
  // Куда идут деньги и комиссия — подпись под кнопками оплаты (SPEC-29 §25), без сети — из снимка.
  const captionMethods = (['cash', 'card', 'transfer', 'invoice'] as const).filter((m) => (payMode === 'mixed' ? Boolean(split[m].trim()) : payMode === m))
  const payCaptions = captionMethods.map((m) =>
    m === 'cash'
      ? payCaption('cash')
      : `${captionMethods.length > 1 ? `${PAYMENT_LABELS[m]} ` : ''}${payCaption(m, bankList.find((b) => b.id === bankFor(m)))}`,
  )
  const payments: { method: PaymentMethod; amount_tyiyn: number; grams?: number; price_per_kg_tyiyn?: number; account_id?: string }[] | null = moneyPayments && [
    ...moneyPayments,
    ...(bonusAmount > 0 ? [{ method: 'bonus' as const, amount_tyiyn: bonusAmount }] : []),
    ...(batteryAmt > 0 && batteryGrams ? [{ method: 'battery' as const, amount_tyiyn: batteryAmt, grams: batteryGrams, price_per_kg_tyiyn: batteryPerKg ?? undefined }] : []),
  ]
  const paid = payments?.reduce((acc, p) => acc + p.amount_tyiyn, 0) ?? null
  // Не деньгами: баллы и аккумулятор уже вычтены из «к оплате».
  const offMoney = bonusAmount + batteryAmt
  // Смешанная оплата: сколько ещё не разнесено по способам.
  const mixedLeft = payMode === 'mixed' && paid !== null ? due - (paid - offMoney) : null
  const receivedValue = payMode === 'cash' && received.trim() ? parseSom(received) : null
  const change = receivedValue !== null ? receivedValue - due : null
  // Пустой чек — сдачи нет, сколько бы ни ввели.
  const shownChange = lines.length > 0 ? change : null

  const update = (key: string, patch: Partial<CartLine>) =>
    setLines((ls) => ls.map((l) => (l.key === key ? { ...l, ...patch } : l)))

  /** Есть ли к этому товару подарки: правило запоминается, спросим, когда наберётся порог. */
  const askGift = (p: Product) => {
    if (giftRules[p.id]) return
    void get<GiftRule[]>(`/gift-rules${qs({ product_id: p.id })}`)
      .then((rules) => {
        const rule = rules.find((r) => r.active && r.items.length > 0)
        if (rule) setGiftRules((m) => ({ ...m, [p.id]: rule }))
      })
      .catch(() => undefined)
  }

  // Порог набран («от 3 л») и подарка из правила в чеке ещё нет — окно «Выберите подарок» (SPEC-11).
  useEffect(() => {
    if (giftRule) return
    const rule = Object.values(giftRules).find(
      (r) =>
        (unitsInCart.get(r.trigger_product_id) ?? 0) >= Math.max(r.min_units, 1) &&
        (unitsInCart.get(r.trigger_product_id) ?? 0) > (giftAsked[r.id] ?? -1) &&
        !lines.some((l) => l.gift && r.items.some((i) => i.gift_product_id === l.product?.id)),
    )
    if (rule) {
      setGiftAsked((a) => ({ ...a, [rule.id]: unitsInCart.get(rule.trigger_product_id) ?? 0 }))
      setGiftRule(rule)
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [lines, giftRules, giftRule])

  const addGift = (g: { product_id: string; qty: number; name: string }) => {
    const key = nextKey()
    setLines((ls) => [
      ...ls,
      {
        key,
        kind: 'piece',
        product: { id: g.product_id, name: g.name } as Product,
        qtyText: String(g.qty),
        priceText: '0',
        gift: true,
      },
    ])
    // Строка встаёт сразу, остаток — когда придёт карточка товара (без сети — из снимка).
    void get<Product>(`/products/${g.product_id}`)
      .catch(async () => (await readSnapshot())?.products.find((p) => p.id === g.product_id) ?? null)
      .then((p) => p && setLines((ls) => ls.map((l) => (l.key === key ? { ...l, product: p } : l))))
      .catch(() => undefined)
    setGiftRule(null)
    focusPicker()
  }

  /** Услуга встаёт строкой, чек становится «в сервис»: работу делает мастер. Цена — по
   *  каталогу с учётом масла в чеке (ADR-096). */
  const addService = (s: Service) => {
    setDone(null)
    setServicesOpen(false)
    setKindsOpen(false)
    setLines((ls) => {
      const same = ls.find((l) => l.kind === 'service' && l.service?.id === s.id)
      if (same) return ls.map((l) => (l === same ? { ...l, qtyText: String((lineQty(same) ?? 0) + 1) } : l))
      return [...ls, { key: nextKey(), kind: 'service', service: s, qtyText: '1', priceText: somInput(listPrice({ kind: 'service', service: s }, hasEngineOil(ls))) }]
    })
    setSaleType('service')
    focusPicker()
  }

  const loadAnalogs = (p: Product) => {
    if (p.category_kind !== 'filter' || analogs[p.id]) return
    void get<{ id: string; name: string; stock_qty: number; sale_price_tyiyn: number }[]>(`/products/${p.id}/analogs`)
      .then((list) => setAnalogs((m) => ({ ...m, [p.id]: list })))
      .catch(() => undefined)
  }

  /** Заменить товар строки аналогом: цена — его, количество — прежнее. */
  const swapToAnalog = (l: CartLine, id: string) =>
    void get<Product>(`/products/${id}`)
      .then((p) => {
        update(l.key, { product: p, priceText: somInput(p.sale_price_tyiyn) })
        loadAnalogs(p)
      })
      .catch((e: unknown) => setError(e instanceof Error ? e.message : 'Не удалось заменить'))

  const addProduct = (p: Product) => {
    setDone(null)
    loadAnalogs(p)
    askGift(p)
    const kind: SaleLineKind = p.unit === 'ml' ? 'container' : p.unit === 'g' ? 'weight' : 'piece'
    setLines((ls) => {
      const same = ls.find((l) => l.product?.id === p.id && l.kind === kind)
      if (same) {
        const n = lineQty(same) ?? 0
        return ls.map((l) => (l === same ? { ...l, qtyText: String(n + 1) } : l))
      }
      return [...ls, { key: nextKey(), kind, product: p, qtyText: '1', priceText: somInput(listPrice({ kind, product: p })) }]
    })
  }

  /** Аккумуляторы на вес: строка по весу, цена — продажная за кг или средняя закупка (ADR-048). */
  const sellBatteries = () =>
    void get<BatteryInfo>('/batteries')
      .then(async (info) => {
        setDone(null)
        setBatteryAvg(info.avg_per_kg_tyiyn)
        const p = await get<Product>(`/products/${info.product_id}`)
        const key = nextKey()
        setLines((ls) => [
          ...ls,
          {
            key,
            kind: 'weight',
            product: p,
            qtyText: '',
            priceText: somInput(info.sale_per_kg_tyiyn || info.avg_per_kg_tyiyn || 0),
          },
        ])
        // Фокус в вес новой строки — в той же отрисовке, где строка появилась.
        focusAfterRender(`[data-weight="${key}"]`)
      })
      .catch((e: unknown) => setError(e instanceof Error ? e.message : 'Не удалось открыть продажу на вес'))

  const switchOil = (l: CartLine, kind: 'container' | 'pour') => {
    if (l.kind === kind) return
    update(l.key, { kind, qtyText: '1', priceText: somInput(listPrice({ kind, product: l.product })) })
  }

  // «В сервис» — сразу выбор вида замены: без него чек не провести (ADR-096).
  const changeType = (t: 'takeaway' | 'service') => {
    setSaleType(t)
    if (t === 'service' && !lines.some((l) => l.kind === 'service')) setKindsOpen(true)
  }

  /** Шаг количества: 1 шт или канистра, 0,5 л для розлива. */
  const step = (l: CartLine, dir: 1 | -1) => {
    const q = lineQty(l) ?? 0
    if (l.kind === 'pour' || l.kind === 'weight') {
      const ml = Math.max(500, q + dir * 500)
      update(l.key, { qtyText: litersInput(ml) })
    } else {
      update(l.key, { qtyText: String(Math.max(1, q + dir)) })
    }
  }

  const reset = () => {
    setLines([])
    setDropped([])
    setRemoved(null)
    setComment('')
    setReceived('')
    setSplit(NO_SPLIT)
    setBonus(null)
    setBatteryPay(null)
    setPayMode('cash')
    setBankPick(NO_BANK_PICK)
    setMasterId('')
    setSaleType('takeaway')
    setParty(null)
    setContactId('')
    setVehicleId('')
    setMileage('')
    setDelivery(null)
    setDeclined('')
    setOilAsked(false)
    setOilQuestion(false)
    setCommentOpen(false)
    setPriceEdit(null)
    setError(null)
    setGiftAsked({})
    attempt.current = null
  }

  // Прошлый пробег машины: лишний ноль уйдёт в книжку, на печать и клиенту в бот.
  const book = useLoad(() => (vehicleId ? get<VehicleBook>(`/vehicles/${vehicleId}/oil-book`) : Promise.resolve(null)), [vehicleId])
  // Пробег — в единице машины (км или мили, ADR-084); прошлый — переведён в неё же.
  // Без связи книжки нет — единица машины из снимка кассы (ADR-093).
  const snapUnit = useLoad(
    () =>
      vehicleId
        ? readSnapshot().then((snap) => snap?.clients?.flatMap((c) => c.vehicles).find((v) => v.id === vehicleId)?.mileage_unit ?? null)
        : Promise.resolve(null),
    [vehicleId],
  )
  const carUnit: MileageUnit = book.data?.mileage_unit ?? snapUnit.data ?? 'km'
  const lastKmRaw = book.data?.records.find((r) => r.mileage_km !== null)?.mileage_km ?? null
  const lastKm = lastKmRaw === null ? null : fromKm(lastKmRaw, carUnit)
  const mileageWarning = (() => {
    const m = mileage ? Number(mileage) : null
    if (m === null || lastKm === null) return null
    if (m < lastKm) return `Меньше, чем в прошлый раз (${mileageText(lastKm, carUnit)}) — проверьте.`
    if (m > lastKm + 50_000) return `В прошлый раз было ${mileageText(lastKm, carUnit)} — точно ${mileageText(m, carUnit)}?`
    return null
  })()

  const debtAmount = payments?.filter((p) => p.method === 'debt').reduce((acc, p) => acc + p.amount_tyiyn, 0) ?? 0
  // На сколько долг выходит за лимит: касса продаёт, но просит владельца поднять (SPEC-10).
  const overLimit =
    party?.credit_limit_tyiyn != null && debtAmount > 0
      ? Math.max(0, party.balance_tyiyn + debtAmount - party.credit_limit_tyiyn)
      : 0
  /** Откладывает текущий чек, возвращая новый список отложенных. */
  const parkCurrent = (list: Parked[]): Parked[] => {
    if (lines.length === 0) return list
    const id = parkedId ?? crypto.randomUUID()
    const item: Parked = {
      id,
      at: new Date().toISOString(),
      label: party?.name ?? '',
      saleType,
      masterId,
      lines,
      comment,
      party,
      contactId,
      vehicleId,
      mileage,
      delivery,
      declined,
      dropped,
    }
    return [...list.filter((p) => p.id !== id), item]
  }

  const park = () => {
    const next = parkCurrent(parked)
    setParked(next)
    saveParked(next)
    setParkedId(null)
    reset()
    focusPicker()
  }

  const resume = (p: Parked) => {
    // Текущий чек не теряется: он уходит в отложенные.
    const next = parkCurrent(parked).filter((x) => x.id !== p.id)
    setParked(next)
    saveParked(next)
    // Сначала всё от текущего чека (оплата, баллы, получено), потом поля отложенного.
    reset()
    setSaleType(p.saleType)
    setMasterId(p.masterId)
    setLines(p.lines)
    setDropped(p.dropped ?? [])
    setComment(p.comment)
    setParty(p.party)
    setContactId(p.contactId)
    setVehicleId(p.vehicleId)
    setMileage(p.mileage ?? '')
    setDelivery(p.delivery ?? null)
    setDeclined(p.declined ?? '')
    setParkedId(p.id)
    setDone(null)
    attempt.current = null
    focusPicker()
  }

  const dropParked = (id: string) => {
    const next = parked.filter((p) => p.id !== id)
    setParked(next)
    saveParked(next)
  }

  // Отменённые товары (SPEC-30 §8): исправление владельцем их не пишет — это не работа кассы.
  const asRemoved = (l: CartLine, reason: CancelReason): RemovedLine | null =>
    fix
      ? null
      : removedLine(
          {
            key: l.key,
            kind: l.kind,
            product_id: l.product?.id ?? null,
            service_id: l.service?.id ?? null,
            name: l.product ? productTitle(l.product) : (l.service?.name ?? ''),
            qty: lineQty(l),
            price: parseSom(l.priceText),
            gift: l.gift,
          },
          reason,
        )
  /** «Очистить» и удаление отложенного: весь чек и удалённые раньше строки — отдельной записью. */
  const cancelAll = (ls: CartLine[], before: RemovedLine[], reason: CancelReason) => {
    if (fix) return
    const all = [...before, ...ls.map((l) => asRemoved(l, reason)).filter((x): x is RemovedLine => x !== null)]
    void sendCancelled(all, shift.data?.id ?? null, user)
  }
  const discardParked = (p: Parked) => {
    cancelAll(p.lines, p.dropped ?? [], 'held')
    dropParked(p.id)
  }

  const invoiceAmount = payments?.filter((p) => p.method === 'invoice').reduce((acc, p) => acc + p.amount_tyiyn, 0) ?? 0
  // Чек «В сервис»: вид замены и данные клиента (ADR-096, ADR-100).
  const serviceKinds = lines.filter((l) => l.kind === 'service')
  const clientNeeds =
    saleType === 'service' ? serviceClientMissing({ party, contactChosen: Boolean(contactId), vehicleChosen: Boolean(vehicleId), mileage }) : []
  // Скидка без права (ADR-097): например, чек отложил владелец, а проводит администратор.
  const discountLines = canDiscount ? [] : lines.filter((l) => isDiscount(parseSom(l.priceText), lineList(l), l.gift))
  const blockers = missingWithFocus(
    [lines.length > 0, 'товары', '[data-picker]'],
    [lines.length === 0 || valid, 'количество и цены в строках'],
    [Boolean(cashierId), 'кассира', '#cashier-select'],
    [saleType !== 'service' || Boolean(masterId), 'мастера', '#master-select'],
    [saleType === 'service' || !lines.some((l) => l.kind === 'service'), 'для услуги — режим «В сервис»'],
    [saleType !== 'service' || serviceKinds.length > 0, 'вид замены', '[data-service-kind]'],
    // Без связи данные клиента не дописать: чек уйдёт, владелец увидит, чего не хватало.
    ...(online ? clientNeeds : []).map((n): [boolean, string, string] =>
      n === 'mileage' ? [false, 'пробег машины — спросите у клиента', '#mileage'] : [false, CLIENT_NEED_LABELS[n], '[data-client]'],
    ),
    [discountLines.length === 0, 'цена ниже каталожной — скидку ставит владелец'],
    [invoiceAmount === 0 || party !== null, 'клиента для оплаты «По счёту»', '[data-client]'],
    [payments !== null, 'сумму оплаты', '[data-pay]'],
    [payments === null || paid === total, `оплату: разнесено ${formatSom((paid ?? 0) - offMoney)} из ${formatSom(due)}`, '[data-pay]'],
    [payMode !== 'cash' || change === null || change >= 0, 'получено меньше итога', '[data-received]'],
    [debtAmount === 0 || party !== null, 'клиента для продажи в долг', '[data-client]'],
    [!bonus || !party || party.id === bonus.party_id, 'баллы другого клиента: уберите скидку или клиента'],
    [!bonus || bonusAmount <= total, 'баллов больше итога — уберите скидку и спишите заново'],
    [!bonus || online, 'баллы списываются только при связи с сервером'],
    [!batteryPay || (batteryGrams !== null && batteryGrams > 0), 'вес сданного аккумулятора', '#battery-kg'],
    [!batteryPay || (batteryPerKg !== null && batteryPerKg > 0), 'цена аккумулятора за кг', '#battery-per-kg'],
    [!batteryPay || online, 'аккумулятор в оплату — только при связи с сервером'],
    [!batteryPay || due >= 0, 'аккумулятор дороже чека — уменьшите вес'],
    [delivery === null || Boolean(delivery.trim()), 'адрес доставки', '#delivery-address'],
    [debtAmount === 0 || !party || Boolean(party.inn.trim()), 'ПИН / ИНН клиента для документа о долге', '#debt-inn'],
    [!fix || Boolean(fixComment.trim()), 'что и почему исправляете', '#correction-comment'],
    [!fix || Boolean(businessDate), 'учётный день', '#correction-date'],
  )

  const submit = () =>
    run(async () => {
      const content = {
        sale_type: saleType,
        cashier_id: cashierId,
        master_id: saleType === 'service' ? masterId : null,
        party_id: party?.id ?? bonus?.party_id ?? null,
        contact_id: party && contactId ? contactId : null,
        vehicle_id: party && vehicleId ? vehicleId : null,
        // Пробег — как вписали, в единице машины (ADR-084); сервер переведёт в км для интервалов.
        mileage: party && vehicleId && saleType === 'service' && mileage ? Number(mileage) : null,
        mileage_unit: party && vehicleId && saleType === 'service' && mileage ? carUnit : null,
        delivery_address: delivery?.trim() ?? '',
        declined: party && vehicleId && saleType === 'service' ? declined.trim() : '',
        comment,
        lines: lines.map((l) => ({
          kind: l.kind,
          gift: Boolean(l.gift),
          product_id: l.product?.id ?? null,
          service_id: l.service?.id ?? null,
          qty: lineQty(l),
          unit_price_tyiyn: parseSom(l.priceText),
          // Прайс, который видела касса: сервер отличит правку кассира от сменившегося прайса.
          seen_list_price_tyiyn: l.gift ? 0 : lineList(l),
        })),
        payments,
      }
      if (fix) {
        // Исправление: сторно исходного и верный чек одной операцией на сервере (SPEC-29 §24).
        const doc = { ...content, business_date: businessDate }
        const fixKey = JSON.stringify([doc, fixComment.trim()])
        if (attempt.current?.key !== fixKey) attempt.current = { key: fixKey, opId: newOpId() }
        const res = await postCorrection({ op_id: attempt.current.opId, kind: 'sale', original_id: fix.originalId, comment: fixComment.trim(), doc })
        toast(`Чек исправлен: № ${res.new_number ?? ''}`)
        navigate(`/sales/${res.new_id}`)
        return
      }
      const key = JSON.stringify(content)
      if (attempt.current?.key !== key) attempt.current = { key, opId: newOpId() }
      const opId = attempt.current.opId
      const body = { op_id: opId, client_time: new Date().toISOString(), ...content, removed_lines: forServer(dropped) }
      // Чек для печати — из того, что видит касса: его можно перепечатать из очереди, пока
      // нет связи, и после отправки (SPEC-09).
      const cashierName = (shiftCashier && cashierId === shiftCashier ? shift.data?.cashier_name : cashiers.find((c) => c.id === cashierId)?.full_name) ?? ''
      const receipt = (temp_no: string): Sale =>
        ({
          id: '',
          number: temp_no,
          kind: 'sale',
          sale_type: saleType,
          cashier_name: cashierName,
          master_name: saleType === 'service' ? (masters.find((m) => m.id === masterId)?.full_name ?? null) : null,
          party_name: party?.name ?? null,
          total_tyiyn: total,
          created_at: new Date().toISOString(),
          lines: lines.map((l, i) => ({
            line_no: i + 1,
            kind: l.kind,
            gift: Boolean(l.gift),
            name: l.product ? productTitle(l.product) : (l.service?.name ?? ''),
            container_ml: l.product?.container_ml ?? null,
            qty: lineQty(l) ?? 0,
            amount_tyiyn: lineAmount(l) ?? 0,
            unit_price_tyiyn: parseSom(l.priceText) ?? 0,
            list_price_tyiyn: lineList(l),
          })),
          payments,
        }) as unknown as Sale
      // Чек сначала в очередь, потом на сервер: обрыв связи его не теряет (SPEC-09).
      const queued = await enqueueSale(opId, body, { user, receipt })
      let sale: Sale
      try {
        sale = await post<Sale>('/sales', body)
      } catch (e) {
        // Ответа нет — обрыв, таймаут или сервер перезапускается (502/503/504), сервер упал (500)
        // или истёк вход (401): чек остаётся в очереди и уйдёт сам; повтор с тем же op_id второй
        // продажи не создаст (ADR-013, ADR-062).
        const noAnswer = e instanceof ApiError ? keepInQueue(e) : !navigator.onLine
        if (!noAnswer) {
          // Сервер ответил отказом: кассир видит ошибку, сам чек позже не уйдёт.
          await removeSale(opId).catch(() => undefined)
          throw e
        }
        await markPending(opId, e instanceof Error ? e.message : 'нет связи').catch(() => undefined)
        // Остаток в снимке уменьшается сразу: следующий покупатель видит, что товара стало меньше.
        void takeFromSnapshot(lines.filter((l) => l.product).map((l) => ({ product_id: l.product?.id ?? '', units: lineUnits(l) })))
        const offlineSale = queued.receipt ?? receipt(queued.temp_no)
        setDone({ sale: offlineSale, change: payMode === 'cash' ? change : null })
        if (parkedId) dropParked(parkedId)
        setParkedId(null)
        reset()
        focusPicker()
        return
      }
      await markSent(opId, sale.number).catch(() => undefined)
      setDone({ sale, change: payMode === 'cash' ? change : null })
      // Замена в сервисе — запись в масляной книжке: показываем и печатаем её клиенту (SPEC-16).
      const bookVehicle = saleType === 'service' && party && vehicleId ? vehicleId : null
      if (bookVehicle) {
        const client = party?.name ?? null
        void get<VehicleBook>(`/vehicles/${bookVehicle}/oil-book`)
          .then((book) => {
            setDone((d) => (d && d.sale.id === sale.id ? { ...d, book } : d))
            if (book.records[0]?.sale_id === sale.id) return printOilBook(book, { client, master: sale.master_name })
          })
          .catch(() => toast('Масляная книжка не напечатана — напечатайте её из карточки клиента'))
      }
      // Долг сразу оформляется бумагой: расписка физлица в двух экземплярах или накладная фирмы (SPEC-10).
      if (sale.party_id && sale.payments.some((p) => p.method === 'debt')) {
        const partyId = sale.party_id
        void Promise.all([
          get<Party>(`/parties/${partyId}`),
          get<DebtDocSettings>('/settings/debt-docs'),
          get<Reconciliation>(`/parties/${partyId}/reconciliation`),
        ])
          .then(([p, settings, history]) => printDebtDoc({ sale, party: p, settings, history }))
          .catch(() => toast('Документ о долге не напечатан — напечатайте его из чека'))
      }
      if (parkedId) dropParked(parkedId)
      setParkedId(null)
      reset()
      focusPicker()
    })

  const canSubmit = !busy && blockers.length === 0
  // На вынос с моторным маслом — сначала «Это замена масла?» (SPEC-29 §18): один раз на чек.
  const askOil = !fix && saleType === 'takeaway' && !oilAsked && hasEngineOil(lines)
  const trySubmit = () => {
    if (askOil) {
      setOilQuestion(true)
      return
    }
    void submit()
  }
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      // Открыто окно (подарок, баллы, клиент, услуги…) — сочетание относится к нему, не к чеку.
      const dialog = Boolean(document.querySelector('[role="dialog"]'))
      if (e.key === 'Enter' && (e.ctrlKey || e.metaKey) && canSubmit && !dialog) {
        e.preventDefault()
        trySubmit()
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  })

  const startShift = () =>
    void get<CashAccount[]>('/cash/accounts')
      .then((accs) => setShiftOpen({ till: accs.find((a) => isTill(a)) }))
      .catch((e: unknown) => setError(e instanceof Error ? e.message : 'Нет связи с сервером: смена открывается только при связи'))

  const pickParty = (p: Party | null) => {
    setParty(p)
    setContactId('')
    setVehicleId('')
    if (!p && (payMode === 'debt' || payMode === 'invoice')) setPayMode('cash')
  }
  const pickVehicle = (v: string) => {
    setVehicleId(v)
    setMileage('')
  }
  const removeLine = (l: CartLine) => {
    const index = lines.findIndex((x) => x.key === l.key)
    setLines((ls) => ls.filter((x) => x.key !== l.key))
    const rec = asRemoved(l, 'remove')
    if (rec) setDropped((d) => [...d, rec])
    if (touch) setRemoved({ line: l, index })
  }
  const undoRemove = () => {
    if (!removed) return
    setLines((ls) => [...ls.slice(0, removed.index), removed.line, ...ls.slice(removed.index)])
    setDropped((d) => unremove(d, removed.line.key))
    setRemoved(null)
  }
  // Кнопки режима кассы (ui-rules §9): под палец, с видимым нажатием, без мелких ссылок.
  const tbtn =
    'inline-flex min-h-[56px] items-center justify-center gap-2 rounded-lg border border-slate-300 bg-white px-3 py-1 text-center text-base leading-tight font-medium text-slate-800 shadow-sm active:bg-slate-100 disabled:text-slate-400'
  const delivering = delivery !== null

  // Ошибка, чего не хватает, «Провести чек» и «Отложить» / «Очистить». В режиме кассы — отдельной
  // карточкой внизу колонки: всегда на экране, что бы ни было выше.
  const submitBlock = (
    <>
      <ErrorBox error={error} />
      <Missing items={blockers} />
      <Button className={touch ? 'min-h-[76px]! w-full shrink-0 flex-wrap gap-x-2! gap-y-0! py-2 text-xl! font-bold! whitespace-nowrap shadow-md sm:text-2xl!' : 'min-h-14! py-3 text-lg!'} disabled={!canSubmit} onClick={trySubmit}>
        {fix ? 'Сохранить исправление' : 'Провести чек'}
        {touch && due > 0 && <span className="font-semibold tabular-nums">· {formatSom(due)}</span>}
      </Button>
      <div className={`hidden text-center text-xs text-slate-400 ${touch ? '' : 'lg:block'}`}>Ctrl + Enter — {fix ? 'сохранить' : 'провести'}</div>
      {!fix && lines.length > 0 && (
        <div className={touch ? 'grid shrink-0 grid-cols-2 gap-2' : 'flex gap-2'}>
          <Button variant="secondary" className={`flex-1 ${touch ? 'min-h-[56px]! text-lg!' : ''}`} onClick={park}>
            Отложить
          </Button>
          {confirmClear ? (
            <Button
              variant="ghost"
              className={`flex-1 text-rose-700 ${touch ? 'min-h-[56px]! border border-rose-300 bg-rose-50 text-lg!' : ''}`}
              onClick={() => {
                setConfirmClear(false)
                cancelAll(lines, dropped, 'clear')
                setParkedId(null)
                setGiftRule(null)
                reset()
              }}
            >
              Точно очистить?
            </Button>
          ) : (
            <Button variant="ghost" className={`flex-1 ${touch ? 'min-h-[56px]! border border-slate-300 text-lg!' : ''}`} onClick={() => setConfirmClear(true)}>
              Очистить
            </Button>
          )}
        </div>
      )}
    </>
  )

  return (
    // Режим кассы на широком экране — ровно во весь экран: страница не прокручивается, прокручивается
    // только список чека (и колонка оплаты, если ей не хватит места).
    <div
      className={`grid ${
        touch
          ? 'grid-cols-1 gap-3 lg:h-full lg:grid-cols-[minmax(0,1fr)_27rem] lg:grid-rows-[auto_minmax(0,1fr)] 2xl:grid-cols-[minmax(0,1fr)_34rem]'
          : 'gap-4 lg:grid-cols-[1fr_380px]'
      }`}
    >
      {touch && (
        <div className="lg:col-span-2">
          <CashierModeBar shift={shift.loading && !shiftKnown ? undefined : shift.data} onOpenShift={startShift} />
        </div>
      )}
      <div className={`flex min-w-0 flex-col ${touch ? 'gap-3 lg:min-h-0' : 'gap-4'}`}>
        {fix && (
          <div className="flex flex-col gap-2">
            <CorrectionBanner label="чек" number={fix.number} comment={fixComment} onComment={setFixComment} />
            <Field label="Учётный день" hint="Как у исходного чека или за последние два дня">
              <input id="correction-date" type="date" className="w-44" value={businessDate} onChange={(e) => setBusinessDate(e.target.value)} />
            </Field>
            <Link to={`/sales/${fix.originalId}`} className="text-sm text-sky-700 hover:underline">
              ← Отменить исправление
            </Link>
          </div>
        )}
        {!fix && parked.length > 0 && (
          <Card className="flex flex-wrap items-center gap-2">
            <span className="text-sm text-slate-500">Отложенные:</span>
            {parked.map((p) => (
              <span
                key={p.id}
                className="inline-flex items-center gap-1 rounded-md border border-slate-300 bg-white px-2 py-1 text-sm"
              >
                <button type="button" className="font-medium hover:text-sky-700" onClick={() => resume(p)}>
                  {p.label || `Чек на ${p.lines.length} поз.`} · {formatSom(parkedTotal(p))}
                </button>
                <button
                  type="button"
                  className="text-slate-400 hover:text-rose-600"
                  aria-label="Удалить отложенный чек"
                  onClick={() => discardParked(p)}
                >
                  ✕
                </button>
              </span>
            ))}
          </Card>
        )}
        {!fix && storageAtRisk && (
          <div className={`rounded-md border border-rose-200 bg-rose-50 px-3 text-sm text-rose-900 ${touch ? 'py-1 lg:line-clamp-1' : 'py-2'}`} role="alert">
            Браузер может стереть неотправленные чеки — разрешите хранение данных для этого сайта в настройках браузера
            (или установите программу на рабочий стол).
          </div>
        )}
        {!fix && !touch && shift.data === null && (
          <div
            className="flex items-center gap-2 rounded-md border border-amber-200 bg-amber-50 px-3 py-1 text-xs text-amber-900"
            title="Продавать можно, но наличные будет не с чем сверить"
          >
            <span className="h-1.5 w-1.5 shrink-0 rounded-full bg-amber-500" aria-hidden="true" />
            <span className="min-w-0 flex-1 truncate">Смена не открыта — наличные будет не с чем сверить</span>
            <Link to="/shift" className="shrink-0 font-medium underline">
              Открыть смену
            </Link>
          </div>
        )}
        <Card className={touch ? 'p-3' : ''}>
          <div className="flex items-start gap-2">
            <div className="min-w-0 flex-1">
              <ProductPicker
                onPick={addProduct}
                onUnknownCode={can(user, 'products') ? setUnknownCode : undefined}
                onCreate={can(user, 'products') ? setNewProductName : undefined}
              />
            </div>
            <Button variant="secondary" className={`shrink-0 ${touch ? 'min-h-[64px]! px-5 text-lg!' : ''}`} onClick={() => setServicesOpen(true)}>
              <svg aria-hidden="true" viewBox="0 0 24 24" className="h-4 w-4" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                <path d="M14.7 6.3a4 4 0 0 0-5.4 5.4L3 18l3 3 6.3-6.3a4 4 0 0 0 5.4-5.4l-2.5 2.5-2.4-.6-.6-2.4z" />
              </svg>
              Услуги
            </Button>
          </div>
          {touch ? (
            // В режиме кассы всё на виду и крупно: без скрытого меню «…» и мелких ссылок.
            <div className="mt-2 grid grid-cols-2 gap-2 sm:grid-cols-4">
              {can(user, 'debt_payment') && (
                <button type="button" className={tbtn} onClick={() => setDebtPay(true)}>
                  <Icon name="debts" className="h-5 w-5 shrink-0" />
                  Принять оплату долга
                </button>
              )}
              {can(user, 'expenses') && (
                <button type="button" className={tbtn} onClick={() => setExpense(true)}>
                  <Icon name="expenses" className="h-5 w-5 shrink-0" />
                  Мелкий расход из кассы
                </button>
              )}
              {can(user, 'battery_intake') && (
                <button type="button" className={tbtn} onClick={() => setBatteryIntake(true)}>
                  Приём аккумуляторов
                </button>
              )}
              <button type="button" className={tbtn} onClick={sellBatteries}>
                Аккумуляторы на вес
              </button>
            </div>
          ) : (
            <div className={`mt-2 flex-wrap items-center gap-x-1 gap-y-1 ${fix ? 'hidden' : 'flex'}`}>
              {can(user, 'debt_payment') && (
                <button type="button" className="inline-flex items-center gap-1.5 rounded-md px-2 py-1.5 text-xs text-slate-600 hover:bg-slate-100 hover:text-slate-900" onClick={() => setDebtPay(true)}>
                  <Icon name="debts" className="h-4 w-4" />
                  Принять оплату долга
                </button>
              )}
              {can(user, 'expenses') && (
                <button type="button" className="inline-flex items-center gap-1.5 rounded-md px-2 py-1.5 text-xs text-slate-600 hover:bg-slate-100 hover:text-slate-900" onClick={() => setExpense(true)}>
                  <Icon name="expenses" className="h-4 w-4" />
                  Мелкий расход из кассы
                </button>
              )}
              <RowMenu
                label="Ещё действия кассы"
                items={[
                  ...(can(user, 'battery_intake') ? [{ label: 'Приём аккумуляторов', onClick: () => setBatteryIntake(true) }] : []),
                  { label: 'Аккумуляторы на вес', onClick: sellBatteries },
                ]}
              />
              <button
                type="button"
                className="ml-auto inline-flex items-center gap-1.5 rounded-md border border-slate-300 px-3 py-1.5 text-xs font-medium text-slate-700 hover:bg-slate-100"
                onClick={() => {
                  setCashierMode(true)
                  enterFullscreen()
                }}
              >
                <Icon name="cashier" className="h-4 w-4" />
                Режим кассы
              </button>
            </div>
          )}
        </Card>

        {touch ? (
          // Плитки не съедают место чека: не выше четверти экрана, дальше прокручиваются.
          <div className="relative lg:max-h-[24dvh] lg:shrink-0 lg:overflow-y-auto lg:overscroll-contain">
            <CashierFavorites compact onProduct={addProduct} onService={addService} />
          </div>
        ) : (
          <CashierFavorites onProduct={addProduct} onService={addService} />
        )}

        {done && (
          <Card className={`border-emerald-300 bg-emerald-50 ${touch ? 'relative p-3 lg:max-h-[30dvh] lg:shrink-0 lg:overflow-y-auto' : ''}`}>
            <div className="flex flex-wrap items-center justify-between gap-3">
              <div>
                <div className="font-semibold text-emerald-800">Чек № {done.sale.number} проведён</div>
                <div className="text-sm text-emerald-900">
                  Итог <Money value={done.sale.total_tyiyn} />
                  {done.change !== null && done.change > 0 && (
                    <>
                      {' '}
                      · сдача <Money value={done.change} />
                    </>
                  )}
                </div>
              </div>
              <div className="flex gap-2">
                <Button variant="secondary" onClick={() => printSale(done.sale, done.change)}>
                  Печать чека
                </Button>
                {done.book && (
                  <Button variant="secondary" onClick={() => done.book && void printOilBook(done.book, { client: done.sale.party_name, master: done.sale.master_name })}>
                    Печать книжки
                  </Button>
                )}
                {done.sale.id ? (
                  <Link className={touch ? tbtn : 'self-center text-sm text-sky-700 hover:underline'} to={`/sales/${done.sale.id}`}>
                    Открыть
                  </Link>
                ) : (
                  <span className="self-center text-xs text-emerald-800">уйдёт на сервер, когда появится сеть</span>
                )}
              </div>
            </div>
            {done.book && done.book.records[0]?.sale_id === done.sale.id && (
              <div className="mt-3 rounded-md border border-emerald-200 bg-white px-3 py-2 text-sm">
                <div className="font-medium">Записано в масляную книжку · {done.book.plate}</div>
                {recordMileage(done.book.records[0]) !== null && (
                  <div className="text-slate-700">пробег {recordMileage(done.book.records[0])}</div>
                )}
                {recordLines(done.book.records[0]).map((l, i) => (
                  <div key={i} className="text-slate-700">
                    {slotLabel(l.slot)}: {l.text}
                  </div>
                ))}
                {done.book.records[0].declined?.trim() && (
                  <div className="text-amber-800">Рекомендовано: {done.book.records[0].declined}</div>
                )}
                {slotsWithNext(done.book).length > 0 ? (
                  slotsWithNext(done.book).map((n) => (
                    <div key={n.slot} className="text-slate-700">
                      Следующая · {n.label}: <b>{slotNextText(n)}</b>
                    </div>
                  ))
                ) : (
                  <div className="text-slate-700">
                    Следующая замена: <b>{nextText(done.book)}</b>
                  </div>
                )}
              </div>
            )}
          </Card>
        )}

        <Card className={`p-0 ${touch ? 'flex flex-col lg:min-h-0 lg:flex-1' : ''}`}>
          {removed && (
            <div className="flex items-center gap-3 border-b border-slate-200 bg-slate-50 px-3 py-2" role="status">
              <span className="min-w-0 flex-1 truncate text-base">
                Удалено: {removed.line.product ? productTitle(removed.line.product) : removed.line.service?.name}
              </span>
              <button type="button" className={tbtn} onClick={undoRemove}>
                Вернуть
              </button>
            </div>
          )}
          <div ref={cartRef} className={touch ? 'relative lg:min-h-0 lg:flex-1 lg:overflow-y-auto lg:overscroll-contain' : ''} data-testid="cart">
          {lines.length === 0 ? (
            <div className={`p-8 text-center text-slate-500 ${touch ? 'text-lg' : 'text-sm'}`}>Чек пуст. Сканируйте товар или найдите его по названию.</div>
          ) : (
            <ul className="divide-y divide-slate-100">
              {lines.map((l, i) => {
                const amount = amounts[i]
                const changed = parseSom(l.priceText) !== lineList(l)
                const priceLabel = l.kind === 'pour' ? 'Цена за л' : l.kind === 'weight' ? 'Цена за кг' : 'Цена'
                const short = l.product ? (unitsInCart.get(l.product.id) ?? 0) > l.product.stock_qty : false
                return (
                  <li
                    key={l.key}
                    // В режиме кассы на широком экране строка — в один ряд: название, количество, цена, сумма, удалить.
                    className={
                      touch
                        ? 'flex flex-wrap items-center gap-x-2 gap-y-2 px-3 py-2 lg:grid lg:grid-cols-[minmax(0,1fr)_auto_auto_auto_auto]'
                        : 'flex flex-wrap items-center gap-2 p-3'
                    }
                  >
                    <div className={touch ? 'min-w-40 flex-1 lg:min-w-0' : 'min-w-48 flex-1'}>
                      <div className={touch ? 'line-clamp-2 text-lg leading-snug font-medium' : 'text-base font-medium'}>
                        {l.product ? productTitle(l.product) : l.service?.name}
                        {l.gift && (
                          <span className="ml-2">
                            <Badge tone="green">подарок</Badge>
                          </span>
                        )}
                      </div>
                      <div className="text-xs text-slate-500">
                        {l.product ? (l.product.stock_qty === undefined ? '' : `Остаток: ${stockText(l.product)}`) : 'Услуга'}
                        {l.product && (analogs[l.product.id] ?? []).length > 0 && (
                          <div className={`mt-1 flex flex-wrap items-center gap-1 ${short ? 'text-rose-700' : 'text-slate-600'}`}>
                            {short ? 'нет на складе — есть аналог:' : 'Аналоги:'}
                            {(analogs[l.product.id] ?? []).slice(0, 3).map((a) => (
                              <button
                                key={a.id}
                                type="button"
                                className="rounded border border-slate-300 bg-white px-1.5 py-0.5 hover:border-sky-500"
                                title="Заменить этим товаром"
                                onClick={() => swapToAnalog(l, a.id)}
                              >
                                {a.name} · {a.stock_qty} шт · {formatSom(a.sale_price_tyiyn)} ⇄
                              </button>
                            ))}
                          </div>
                        )}
                        {l.kind === 'weight' && batteryAvg !== null && (
                          <span className="ml-2 text-amber-700">средняя закупка {formatSom(batteryAvg)} за кг — дешевле не продать</span>
                        )}
                        {short && (
                          <span className="ml-2">
                            <Badge tone="rose">больше, чем на складе</Badge>
                          </span>
                        )}
                        {l.product?.needs_review && (
                          <span className="ml-2">
                            <Badge tone="amber">проверить</Badge>
                          </span>
                        )}
                      </div>
                      {l.product?.unit === 'ml' && (
                        <div className="mt-1 inline-flex overflow-hidden rounded-md border border-slate-300 text-xs">
                          {(['container', 'pour'] as const).map((k) => (
                            <button
                              key={k}
                              type="button"
                              className={`px-2 py-1 ${l.kind === k ? 'bg-sky-600 text-white' : 'bg-white hover:bg-slate-50'}`}
                              onClick={() => switchOil(l, k)}
                            >
                              {k === 'container' ? `Канистра ${l.product?.container_ml ? formatLiters(l.product.container_ml) : ''}` : 'Розлив, л'}
                            </button>
                          ))}
                        </div>
                      )}
                    </div>
                    <label className="flex flex-col text-xs text-slate-500">
                      {/* В режиме кассы подпись только у литров и веса: штуки и канистры понятны и так. */}
                      <span className={touch && l.kind !== 'pour' && l.kind !== 'weight' ? 'sr-only' : ''}>
                        {l.kind === 'pour' ? 'Литры' : l.kind === 'weight' ? 'Вес, кг' : l.kind === 'container' ? 'Канистры' : 'Кол-во'}
                      </span>
                      <span className={`flex items-stretch ${touch ? 'gap-1' : ''}`}>
                        <button
                          type="button"
                          className={qtyBtn(touch, 'l')}
                          aria-label="Меньше"
                          onClick={() => step(l, -1)}
                        >
                          −
                        </button>
                        <input
                          className={`min-h-12 text-center text-lg ${touch ? 'w-[4.5rem] rounded-lg text-xl font-semibold' : 'w-16 rounded-none'}`}
                          data-weight={l.kind === 'weight' ? l.key : undefined}
                          // Подпись вокруг поля относится к первой кнопке «−», поэтому у поля своя.
                          aria-label={l.kind === 'pour' ? 'Литры' : l.kind === 'weight' ? 'Вес, кг' : l.kind === 'container' ? 'Канистры' : 'Кол-во'}
                          {...(l.kind === 'pour' || l.kind === 'weight'
                            ? num('decimal', { decimals: 3, quick: l.kind === 'pour' ? ['1', '4', '5'] : undefined })
                            : num('int'))}
                          value={l.qtyText}
                          onChange={(e) => update(l.key, { qtyText: e.target.value })}
                        />
                        <button
                          type="button"
                          className={qtyBtn(touch, 'r')}
                          aria-label="Больше"
                          onClick={() => step(l, 1)}
                        >
                          +
                        </button>
                      </span>
                    </label>
                    {l.gift ? (
                      <div className="w-28 text-xs text-emerald-700">бесплатно</div>
                    ) : (
                      <div className="flex flex-col text-xs text-slate-500">
                        <label htmlFor={`price-${l.key}`} className={touch && l.kind !== 'pour' && l.kind !== 'weight' ? 'sr-only' : ''}>
                          {priceLabel}
                        </label>
                        <span className="flex items-center gap-0.5">
                          {/* Цена видна текстом; карандаш или касание по ней — правка в том же поле. */}
                          <input
                            id={`price-${l.key}`}
                            aria-label={priceLabel}
                            className={`${touch ? 'w-24 text-lg' : 'w-20'} ${
                              changed
                                ? 'border-amber-400 bg-amber-50'
                                : priceEdit === l.key
                                  ? ''
                                  : 'cursor-pointer border-transparent bg-transparent text-right font-medium text-slate-800 shadow-none'
                            }`}
                            // Без права на скидку цену не правят: ни карандаша, ни клавиатуры (ADR-097).
                            {...(canDiscount ? num('decimal') : {})}
                            readOnly={!canDiscount}
                            value={l.priceText}
                            onFocus={(e) => {
                              if (!canDiscount) return
                              setPriceEdit(l.key)
                              e.target.select()
                            }}
                            onBlur={() => setPriceEdit((k) => (k === l.key ? null : k))}
                            onChange={(e) => update(l.key, { priceText: e.target.value })}
                            title={changed ? `Прайс: ${formatSom(lineList(l))}` : canDiscount ? 'Изменить цену' : 'Цену ниже каталожной ставит владелец'}
                          />
                          {/* В режиме кассы карандаша нет: касание по цене — уже правка. Без права на скидку — нет нигде. */}
                          <button
                            type="button"
                            hidden={touch || !canDiscount}
                            // Значок мал, а цель нажатия — не меньше 42 точек (ui-rules §5): невидимая рамка вокруг.
                            className="relative rounded p-1 text-slate-400 after:absolute after:-inset-2.5 after:content-[''] hover:bg-slate-100 hover:text-sky-700"
                            aria-label="Изменить цену"
                            onClick={() => document.getElementById(`price-${l.key}`)?.focus()}
                          >
                            <Icon name="edit" className="h-3.5 w-3.5" />
                          </button>
                        </span>
                        {/* В режиме кассы подсказок по наведению нет — прайс виден подписью. */}
                        {touch && changed && <span className="text-amber-700">прайс {formatSom(lineList(l))}</span>}
                      </div>
                    )}
                    <div className={touch ? 'min-w-24 text-right text-xl font-bold' : 'w-24 text-right text-base font-semibold'}>{amount === null ? '—' : <Money value={amount} />}</div>
                    <button
                      type="button"
                      className={
                        touch
                          ? 'flex min-h-[56px] min-w-[56px] items-center justify-center rounded-lg border border-rose-200 bg-rose-50 text-2xl text-rose-700 active:bg-rose-100'
                          : 'flex min-h-12 min-w-12 items-center justify-center rounded-md text-lg text-slate-400 hover:bg-rose-50 hover:text-rose-600'
                      }
                      aria-label="Удалить строку"
                      onClick={() => removeLine(l)}
                    >
                      ✕
                    </button>
                  </li>
                )
              })}
            </ul>
          )}
          </div>
        </Card>
      </div>

      {/* В режиме кассы оплата и «Провести чек» — сверху колонки; клиент, мастер и редкое — под ними,
          в своей прокрутке: колонка помещается в экран. */}
      <div className={`flex flex-col ${touch ? 'gap-3 lg:min-h-0' : 'gap-4'}`}>
        <div
          // В режиме кассы оплата и клиент — одна карточка со своей прокруткой, кнопка «Провести» — под ней.
          className={`flex flex-col ${touch ? 'relative gap-2.5 rounded-lg border border-slate-200 bg-white p-2.5 shadow-sm lg:min-h-0 lg:flex-1 lg:overflow-y-auto lg:overscroll-contain' : 'gap-4'}`}
          data-testid="pay-column"
        >
        <Card className={`flex flex-col ${touch ? 'order-2 gap-2.5 rounded-none! border-0! border-t! border-slate-200! p-0! pt-2.5! shadow-none!' : 'gap-3'}`}>
          {!touch && (
            <div className="grid grid-cols-2 overflow-hidden rounded-md border border-slate-300 text-base">
              {(['takeaway', 'service'] as const).map((t) => (
                <button
                  key={t}
                  type="button"
                  className={`min-h-12 py-2 text-base ${saleType === t ? 'bg-sky-600 text-white' : 'bg-white hover:bg-slate-50'}`}
                  onClick={() => changeType(t)}
                >
                  {t === 'takeaway' ? 'На вынос' : 'В сервис'}
                </button>
              ))}
            </div>
          )}
          {shift.data && !fix ? (
            <div className={`text-sm ${touch ? 'hidden' : ''}`} data-testid="shift-cashier">
              Кассир: <b>{shift.data.cashier_name}</b> <span className="text-slate-500">(смена № {shift.data.number})</span>
            </div>
          ) : (
            (shiftKnown || fix) && (
              <Field label="Кассир" required hint={fix ? 'Кассир исходного чека: ему начислится процент с исправленного' : 'Запомнится на этом устройстве до открытия смены'}>
                <select
                  id="cashier-select"
                  value={cashierId}
                  onChange={(e) => {
                    setCashierId(e.target.value)
                    if (!fix) remember(CASHIER_KEY, e.target.value)
                  }}
                >
                  <option value="">— выберите —</option>
                  {cashiers.map((e) => (
                    <option key={e.id} value={e.id}>
                      {e.full_name}
                    </option>
                  ))}
                </select>
              </Field>
            )
          )}
          {saleType === 'service' && (
            <>
              <Field label="Мастер" required>
                <select id="master-select" value={masterId} onChange={(e) => setMasterId(e.target.value)}>
                  <option value="">— выберите —</option>
                  {masters.map((e) => (
                    <option key={e.id} value={e.id}>
                      {e.full_name}
                    </option>
                  ))}
                </select>
              </Field>
              {/* Вид замены обязателен: мастеру — сумма ставок видов замены чека (ADR-096). */}
              <div className="flex flex-col gap-1" data-testid="service-kinds">
                <div className="flex items-center justify-between gap-2">
                  <span className="text-sm font-medium text-slate-700">
                    Вид замены <span className="text-rose-600">*</span>
                  </span>
                  <button type="button" data-service-kind className={touch ? tbtn : 'min-h-9 rounded-md border border-slate-300 px-3 text-sm hover:bg-slate-50'} onClick={() => setKindsOpen(true)}>
                    {serviceKinds.length > 0 ? '+ ещё вид' : 'Выбрать вид замены'}
                  </button>
                </div>
                {serviceKinds.length === 0 ? (
                  <div className="text-sm text-rose-700">Не выбран — без него чек «В сервис» не провести.</div>
                ) : (
                  <div className={touch ? 'text-base' : 'text-sm'}>
                    {serviceKinds.map((l) => (
                      <div key={l.key}>
                        {l.service?.name}
                        {(lineQty(l) ?? 1) > 1 && ` × ${lineQty(l)}`} ·{' '}
                        {lineList(l) === 0 && l.service?.free_with_oil ? 'бесплатно — масло наше' : formatSom(lineAmount(l) ?? 0)}
                      </div>
                    ))}
                    <div className="text-xs text-slate-500">
                      Мастеру начислится {formatSom(serviceKinds.reduce((acc, l) => acc + (l.service?.master_fee_tyiyn ?? 0) * (lineQty(l) ?? 0), 0))} — ставки
                      видов замены.
                    </div>
                  </div>
                )}
              </div>
            </>
          )}
          {touch ? (
            // Клиент в режиме кассы — короткой строкой; поиск и машина — в окне, где есть место списку.
            <div className="flex items-center gap-2" data-testid="client-row">
              <div className="min-w-0 flex-1">
                <div className="text-sm text-slate-500">Клиент</div>
                <div className="truncate text-lg font-medium">
                  {party ? party.name : <span className="text-slate-400">не выбран</span>}
                  {party && <span className="text-base font-normal text-slate-500"> · {vehicleId ? 'машина выбрана' : 'машина не указана'}</span>}
                </div>
              </div>
              <button
                type="button"
                data-client
                className={tbtn}
                onClick={() => {
                  setClientOpen(true)
                  if (!party) focusAfterRender('[data-client-input]')
                }}
              >
                {party ? 'Изменить' : 'Выбрать клиента'}
              </button>
              {party && (
                <button type="button" className={tbtn} aria-label="Убрать клиента" onClick={() => pickParty(null)}>
                  ✕
                </button>
              )}
            </div>
          ) : (
            <div data-client>
              <ClientPicker party={party} contactId={contactId} vehicleId={vehicleId} onParty={pickParty} onContact={setContactId} onVehicle={pickVehicle} />
            </div>
          )}
          {party && vehicleId && !touch && <OilHint vehicleId={vehicleId} />}
          {/* Чек «В сервис» — с данными клиента (ADR-100): что не хватает — со звёздочкой; имя и
              телефон дописываются в карточку прямо здесь. */}
          {saleType === 'service' && clientNeeds.length > 0 && (
            <div className="flex flex-col gap-2 rounded-md border border-rose-200 bg-rose-50 px-3 py-2 text-sm text-rose-900" data-testid="service-client-needs">
              <div>
                Для замены нужны:{' '}
                {clientNeeds.map((n, i) => (
                  <span key={n}>
                    {i > 0 && ', '}
                    {CLIENT_NEED_LABELS[n]} <span className="font-semibold text-rose-600">*</span>
                  </span>
                ))}
                {!online && <div className="text-xs">Нет связи: чек уйдёт без этих данных, владелец увидит, чего не хватало.</div>}
              </div>
              {party && online && ((clientNeeds.includes('client') && party.kind === 'person') || clientNeeds.includes('phone')) && (
                <div className="flex flex-wrap items-end gap-2">
                  {clientNeeds.includes('client') && party.kind === 'person' && (
                    <>
                      <Field label="Имя" required>
                        <input id="client-first-name" className="w-32" value={clientForm.first} onChange={(e) => setClientForm({ ...clientForm, first: e.target.value })} />
                      </Field>
                      <Field label="Фамилия" required>
                        <input id="client-last-name" className="w-36" value={clientForm.last} onChange={(e) => setClientForm({ ...clientForm, last: e.target.value })} />
                      </Field>
                    </>
                  )}
                  {clientNeeds.includes('phone') && (
                    <Field label="Телефон" required>
                      <input id="client-phone" className="w-40" inputMode="tel" value={clientForm.phone} onChange={(e) => setClientForm({ ...clientForm, phone: e.target.value })} />
                    </Field>
                  )}
                  <Button
                    variant="secondary"
                    disabled={
                      clientFix.busy ||
                      (clientNeeds.includes('client') && party.kind === 'person' && (!clientForm.first.trim() || !clientForm.last.trim())) ||
                      (clientNeeds.includes('phone') && !clientForm.phone.trim())
                    }
                    onClick={() =>
                      void clientFix.run(async () => {
                        const names = clientNeeds.includes('client') && party.kind === 'person'
                        const saved = await patch<Party>(`/parties/${party.id}`, {
                          ...(names ? { first_name: clientForm.first.trim(), last_name: clientForm.last.trim() } : {}),
                          ...(clientNeeds.includes('phone') ? { phone: clientForm.phone.trim() } : {}),
                        })
                        setParty({ ...party, ...saved })
                        setClientForm({ first: '', last: '', phone: '' })
                        toast('Записано в карточку клиента')
                      })
                    }
                  >
                    Записать в карточку
                  </Button>
                </div>
              )}
              <ErrorBox error={clientFix.error} />
            </div>
          )}
          {saleType === 'service' &&
            (party && vehicleId ? (
              <>
                <Field
                  label={`Пробег сейчас, ${unitLabel(carUnit)}`}
                  required
                  hint="Спросите у клиента или посмотрите на приборной панели — запишется в масляную книжку"
                >
                  {/* Не больше 7 цифр: пробег до 5 000 000, лишняя цифра — опечатка (ADR-093). */}
                  <input id="mileage" {...num('int')} maxLength={7} value={mileage} onChange={(e) => setMileage(e.target.value.replace(/\D/g, '').slice(0, 7))} />
                  {mileageWarning && <span className="mt-1 block text-xs text-amber-700">{mileageWarning}</span>}
                </Field>
              </>
            ) : (
              <div className="rounded-md bg-sky-50 px-3 py-2 text-xs text-sky-900">
                Масляная книжка: {party ? 'выберите машину клиента (или «+ новая машина»)' : 'выберите клиента и машину'} — замена, масло, фильтр и пробег
                запишутся сами, клиенту распечатается книжка.
              </div>
            ))}
          {saleType === 'service' && party && vehicleId && (
            <Field label="Мастер рекомендовал (клиент отказался)" hint="Необязательно. Запишется в книжку, в следующий раз касса напомнит">
              <input id="declined" maxLength={300} value={declined} onChange={(e) => setDeclined(e.target.value)} placeholder="Например: воздушный фильтр, антифриз" />
            </Field>
          )}
          {touch ? (
            // Редкое — кнопками: доставка, комментарий, аккумулятор и баллы в оплату.
            <div className="grid grid-cols-2 gap-2 sm:grid-cols-4 lg:grid-cols-2 2xl:grid-cols-4">
              <button
                type="button"
                aria-pressed={delivering}
                className={`${tbtn} ${delivering ? 'border-sky-600! bg-sky-50! text-sky-800!' : ''}`}
                onClick={() => setDelivery(delivering ? null : '')}
              >
                {delivering ? '✓ Доставка' : 'Доставка'}
              </button>
              <button type="button" className={tbtn} disabled={commentOpen || Boolean(comment)} onClick={() => setCommentOpen(true)}>
                Комментарий
              </button>
              <button
                type="button"
                className={tbtn}
                disabled={total <= 0 || Boolean(batteryPay) || batteryLoad.busy || !online}
                onClick={() =>
                  void batteryLoad.run(async () => {
                    const info = await get<BatteryInfo>('/batteries')
                    setBatteryPay({ kgText: '', perKgText: somInput(info.intake_per_kg_tyiyn) })
                    focusAfterRender('#battery-kg')
                  })
                }
              >
                <span>
                  Сдал аккумулятор
                  {!online && <span className="block text-sm font-normal">нет связи</span>}
                </span>
              </button>
              <button type="button" className={tbtn} disabled={total <= 0 || Boolean(bonus)} onClick={() => setBonusOpen(true)}>
                <Icon name="star" className="h-5 w-5 shrink-0" />
                Баллы клиента
              </button>
            </div>
          ) : (
            <div className="flex flex-wrap items-center gap-x-4 gap-y-1 text-xs">
              <label className="inline-flex cursor-pointer items-center gap-1.5 text-sky-700 max-lg:min-h-[42px]">
                <input type="checkbox" className="h-3.5 w-3.5 max-lg:h-5 max-lg:w-5" checked={delivery !== null} onChange={(e) => setDelivery(e.target.checked ? '' : null)} />
                Доставка (бесплатно)
              </label>
              {!commentOpen && !comment && (
                <button type="button" className="text-sky-700 hover:underline max-lg:min-h-[42px]" onClick={() => setCommentOpen(true)}>
                  + комментарий
                </button>
              )}
            </div>
          )}
          {delivery !== null && (
            <Field label="Адрес доставки" required>
              <input id="delivery-address" value={delivery} onChange={(e) => setDelivery(e.target.value)} placeholder="Улица, дом, ориентир" />
            </Field>
          )}
          {(commentOpen || comment) && (
            <Field label="Комментарий">
              <input autoFocus={commentOpen && !comment} value={comment} onChange={(e) => setComment(e.target.value)} />
            </Field>
          )}
          {employees.data && cashiers.length === 0 && (
            <div className="text-sm text-amber-700">
              В справочнике нет кассиров. <Link className="underline" to="/employees">Добавить сотрудника</Link>
            </div>
          )}
        </Card>

        <Card className={`flex flex-col ${touch ? 'order-1 gap-2 rounded-none! border-0! p-0! shadow-none!' : 'gap-3'}`}>
          {touch ? (
            <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
              <div className="grid w-full shrink-0 grid-cols-2 gap-1 rounded-xl bg-slate-100 p-0.5 sm:w-[56%]">
                {(['takeaway', 'service'] as const).map((t) => (
                  <button
                    key={t}
                    type="button"
                    aria-pressed={saleType === t}
                    className={`min-h-[56px] rounded-lg px-1 text-lg font-semibold ${saleType === t ? 'bg-sky-600 text-white shadow' : 'text-slate-700 active:bg-white'}`}
                    onClick={() => changeType(t)}
                  >
                    {t === 'takeaway' ? 'На вынос' : 'В сервис'}
                  </button>
                ))}
              </div>
              <div className="flex min-w-0 flex-1 items-baseline justify-end gap-2">
                <span className="text-sm text-slate-500">Итого</span>
                <Money value={total} className="truncate text-3xl leading-tight font-bold" />
              </div>
            </div>
          ) : (
            <div className="flex items-baseline justify-between">
              <span className="text-sm text-slate-500">Итого</span>
              <Money value={total} className="text-3xl font-bold" />
            </div>
          )}
          {bonus && (
            <div className="flex items-center justify-between gap-2 rounded-md bg-sky-50 px-3 py-2 text-sm text-sky-900">
              <span>
                Баллами ({bonus.name}): −<Money value={bonusAmount} />
                <span className="ml-2 font-semibold">
                  к оплате <Money value={due} />
                </span>
              </span>
              <Button variant="ghost" className="px-2 py-1 text-xs" onClick={() => setBonus(null)}>
                Убрать
              </Button>
            </div>
          )}
          {touch ? (
            // 3 + 3 отдельными крупными кнопками со значками: наличные, карта, QR сверху — частые.
            <div data-pay tabIndex={-1} className="grid grid-cols-3 gap-2">
              {PAY_MODES.filter(payAllowed).map((m) => (
                <button
                  key={m}
                  type="button"
                  aria-pressed={payMode === m}
                  className={`flex min-h-[64px] items-center justify-center gap-1.5 rounded-lg border-2 px-1.5 py-1 text-[15px] leading-tight font-semibold break-words hyphens-auto shadow-sm sm:text-[17px] ${
                    payMode === m ? 'border-sky-600 bg-sky-600 text-white' : 'border-slate-300 bg-white text-slate-800 active:bg-slate-100'
                  }`}
                  onClick={() => setPayMode(m)}
                >
                  <Icon name={PAY_ICONS[m]} className="h-6 w-6 shrink-0 max-sm:hidden" />
                  <span className="min-w-0">{m === 'mixed' ? 'Смешанная' : PAYMENT_LABELS[m]}</span>
                </button>
              ))}
            </div>
          ) : (
            <div data-pay tabIndex={-1} className="grid grid-cols-3 gap-px overflow-hidden rounded-md border border-slate-300 bg-slate-300 text-base">
              {PAY_MODES.filter(payAllowed).map((m) => (
                <button
                  key={m}
                  type="button"
                  // 3 + 3: наличные, карта, QR сверху — частые; «По счёту», долг и смешанная ниже.
                  aria-pressed={payMode === m}
                  className={`min-h-12 py-2 ${payMode === m ? 'bg-sky-600 text-white' : 'bg-white hover:bg-slate-50'}`}
                  onClick={() => setPayMode(m)}
                >
                  {m === 'mixed' ? 'Смешанная' : PAYMENT_LABELS[m]}
                </button>
              ))}
            </div>
          )}
          {/* В режиме кассы «→ Касса · наличные» не пишем: наличные всегда в кассу. */}
          {payCaptions.length > 0 && !(touch && payMode === 'cash') && (
            <div className="-mt-1 flex flex-col text-xs text-slate-500" data-testid="pay-dest">
              {payCaptions.map((c) => (
                <span key={c}>{c}</span>
              ))}
            </div>
          )}
          {!touch && total > 0 && (!bonus || !batteryPay) && (
            <div className="-mt-1 flex flex-wrap justify-end gap-x-4 gap-y-1">
              {!batteryPay && (
                <button
                  type="button"
                  className="inline-flex items-center gap-1 text-xs text-sky-700 hover:underline disabled:cursor-not-allowed disabled:text-slate-400 disabled:no-underline max-lg:min-h-[42px]"
                  disabled={batteryLoad.busy || !online}
                  title={online ? 'Старый аккумулятор в счёт оплаты: вес × цена приёма за кг' : 'Нет связи с сервером: цену приёма считает сервер'}
                  onClick={() =>
                    void batteryLoad.run(async () => {
                      const info = await get<BatteryInfo>('/batteries')
                      setBatteryPay({ kgText: '', perKgText: somInput(info.intake_per_kg_tyiyn) })
                      focusAfterRender('#battery-kg')
                    })
                  }
                >
                  Сдал аккумулятор
                </button>
              )}
              {!bonus && (
                <button type="button" className="inline-flex items-center gap-1 text-xs text-sky-700 hover:underline max-lg:min-h-[42px]" onClick={() => setBonusOpen(true)}>
                  <Icon name="star" className="h-3.5 w-3.5" />
                  Баллы клиента
                </button>
              )}
            </div>
          )}
          <ErrorBox error={batteryLoad.error} />
          {batteryPay && (
            <div className="flex flex-col gap-2 rounded-md bg-sky-50 px-3 py-2 text-sm text-sky-900" data-testid="battery-pay">
              <div className="flex items-end gap-2">
                <Field label="Сдал аккумулятор, вес кг">
                  <input
                    id="battery-kg"
                    className="w-24"
                    {...num('decimal', { decimals: 3 })}
                    value={batteryPay.kgText}
                    onChange={(e) => setBatteryPay({ ...batteryPay, kgText: e.target.value })}
                  />
                </Field>
                <span className="pb-2">×</span>
                <Field label="Цена за кг, с">
                  <input
                    id="battery-per-kg"
                    className="w-24"
                    {...num('decimal')}
                    value={batteryPay.perKgText}
                    onChange={(e) => setBatteryPay({ ...batteryPay, perKgText: e.target.value })}
                  />
                </Field>
                <span className="pb-2">
                  = <b>{formatSom(batteryAmt)}</b>
                </span>
                <Button variant="ghost" className="ml-auto px-2 py-1 text-xs" onClick={() => setBatteryPay(null)}>
                  Убрать
                </Button>
              </div>
              {batteryAmt > 0 && (
                <div className="font-semibold">
                  к оплате <Money value={due} />
                </div>
              )}
            </div>
          )}
          {payMode === 'cash' && (
            <div className={touch ? 'grid grid-cols-2 gap-2' : 'flex flex-col gap-2'}>
              {touch ? (
                // Подпись внутри поля: строка «Получено / Сдача» в одну высоту с кнопками.
                <label className="relative flex flex-col">
                  <span className="pointer-events-none absolute top-1.5 left-3 text-sm text-slate-500">Получено, с</span>
                  <input
                    data-received
                    className="min-h-[64px] w-full pt-5 text-2xl font-semibold"
                    {...num('decimal', { quick: due > 0 ? [somInput(due), ...NOTES.filter((n) => n > due).slice(0, 2).map(somInput)] : undefined })}
                    value={received}
                    onChange={(e) => setReceived(e.target.value)}
                    onKeyDown={(e) => {
                      if (e.key === 'Enter' && canSubmit) trySubmit()
                    }}
                  />
                </label>
              ) : (
                <Field label="Получено, с">
                  <input
                    data-received
                    className="min-h-12 text-lg"
                    {...num('decimal', { quick: due > 0 ? [somInput(due), ...NOTES.filter((n) => n > due).slice(0, 2).map(somInput)] : undefined })}
                    value={received}
                    onChange={(e) => setReceived(e.target.value)}
                    onKeyDown={(e) => {
                      if (e.key === 'Enter' && canSubmit) trySubmit()
                    }}
                  />
                </Field>
              )}
              <div
                className={`flex rounded-md px-3 py-2 ${touch ? 'flex-col justify-center' : 'items-baseline justify-between'} ${
                  shownChange === null ? 'bg-slate-50 text-slate-500' : shownChange >= 0 ? 'bg-emerald-50 text-emerald-800' : 'bg-rose-50 text-rose-800'
                }`}
              >
                <span className="text-sm">Сдача</span>
                {shownChange === null ? (
                  <span className="text-2xl font-semibold">—</span>
                ) : shownChange >= 0 ? (
                  <Money value={shownChange} className="text-2xl font-semibold" />
                ) : (
                  <span className="text-sm font-medium">
                    не хватает <Money value={-shownChange} />
                  </span>
                )}
              </div>
              {due > 0 && (
                <div className={touch ? 'col-span-2 flex gap-2 *:flex-1' : 'flex flex-wrap gap-1.5'}>
                  <button
                    type="button"
                    className={touch ? tbtn : 'min-h-12 rounded-md border border-slate-300 px-3 py-2 text-sm hover:bg-slate-50'}
                    onClick={() => setReceived(somInput(due))}
                  >
                    Без сдачи
                  </button>
                  {NOTES.filter((n) => n > due)
                    .slice(0, 3)
                    .map((n) => (
                      <button
                        key={n}
                        type="button"
                        className={touch ? `${tbtn} text-lg! font-semibold!` : 'min-h-12 rounded-md border border-slate-300 px-3 py-2 text-sm hover:bg-slate-50'}
                        onClick={() => setReceived(somInput(n))}
                      >
                        {formatSom(n)}
                      </button>
                    ))}
                </div>
              )}
            </div>
          )}
          {(payMode === 'invoice' || invoiceAmount > 0) && !party && (
            <div className="rounded-md bg-amber-50 px-3 py-2 text-sm text-amber-800" data-testid="invoice-needs-client">
              По счёту платит клиент — выберите его выше: счёт выставляется фирме или человеку.
            </div>
          )}
          {(payMode === 'debt' || debtAmount > 0) && (
            <div className="rounded-md bg-amber-50 px-3 py-2 text-sm text-amber-800">
              {party ? (
                <>
                  В долг {formatSom(debtAmount || due)}: {party.name}, сейчас {balanceText(party.balance_tyiyn)}
                  {party.credit_limit_tyiyn !== null && (
                    <> · осталось по лимиту {formatSom(party.credit_limit_tyiyn - party.balance_tyiyn - (debtAmount || due))}</>
                  )}
                  {overLimit > 0 && (
                    <div className="mt-2 flex flex-wrap items-center gap-2">
                      <span>Лимит превышен на {formatSom(overLimit)}.</span>
                      <Button
                        variant="secondary"
                        className="px-2 py-1 text-xs"
                        disabled={limitAsk.busy}
                        onClick={() =>
                          void limitAsk.run(async () => {
                            await post(`/parties/${party.id}/limit-request`, { amount_tyiyn: overLimit })
                            toast('Владельцу отправлено уведомление')
                          })
                        }
                      >
                        Запросить повышение
                      </Button>
                    </div>
                  )}
                </>
              ) : (
                'Выберите клиента выше — без него долг не записать'
              )}
            </div>
          )}
          {debtAmount > 0 && party && !party.inn.trim() && (
            <div className="flex flex-col gap-2 rounded-md border border-amber-300 bg-amber-50 p-3 text-sm">
              <div className="text-amber-900">
                Для документа о долге нужен {party.kind === 'company' ? 'ИНН фирмы' : 'ПИН клиента (14 цифр с паспорта)'}.
              </div>
              <div className="flex gap-2">
                <input
                  id="debt-inn"
                  className="min-w-0 flex-1"
                  {...num('digits')}
                  placeholder={party.kind === 'company' ? 'ИНН' : 'ПИН'}
                  value={innText}
                  onChange={(e) => setInnText(e.target.value)}
                />
                <Button
                  variant="secondary"
                  disabled={innSave.busy || !/^\d{10,14}$/.test(innText.trim())}
                  onClick={() =>
                    void innSave.run(async () => {
                      const saved = await patch<Party>(`/parties/${party.id}`, { inn: innText.trim() })
                      setParty({ ...party, inn: saved.inn })
                      setInnText('')
                      toast('Записано в карточку клиента')
                    })
                  }
                >
                  Записать
                </Button>
              </div>
              <ErrorBox error={innSave.error} />
            </div>
          )}
          {bankList.length > 1 && noncashMethods.length > 0 && (
            <div className="flex flex-col gap-2 text-sm" data-testid="pay-bank">
              {noncashMethods.map((m) => (
                <div key={m} className="flex flex-wrap items-center gap-2" data-testid={`pay-bank-${m}`}>
                  <span className="text-slate-600">{noncashMethods.length > 1 ? `Банк (${PAYMENT_LABELS[m]}):` : 'Банк:'}</span>
                  {bankChoices(bankList, m).map((b) => (
                    <button
                      key={b.id}
                      type="button"
                      aria-pressed={bankFor(m) === b.id}
                      className={`${touch ? 'min-h-[56px] rounded-lg px-4 text-base font-medium' : 'min-h-9 rounded-md px-3 py-1'} border ${bankFor(m) === b.id ? 'border-sky-600 bg-sky-600 text-white' : 'border-slate-300 bg-white hover:border-sky-500'}`}
                      onClick={() => setBankPick({ ...bankPick, [m]: b.id })}
                    >
                      {b.name}
                    </button>
                  ))}
                </div>
              ))}
            </div>
          )}
          {payMode === 'mixed' && (
            <div className="flex flex-col gap-2">
              <div className="grid grid-cols-2 gap-2 sm:grid-cols-3">
                {SPLIT_METHODS.filter(payAllowed).map((m) => (
                  <Field key={m} label={PAYMENT_LABELS[m]}>
                    <input {...num('decimal')} aria-label={PAYMENT_LABELS[m]} value={split[m]} onChange={(e) => setSplit({ ...split, [m]: e.target.value })} />
                  </Field>
                ))}
              </div>
              {mixedLeft !== null && (
                <div
                  className={`flex flex-wrap items-center justify-between gap-2 rounded-md px-3 py-2 text-sm ${mixedLeft === 0 ? 'bg-emerald-50 text-emerald-800' : 'bg-slate-50 text-slate-700'}`}
                >
                  <span>
                    {mixedLeft === 0 ? 'Разнесено полностью' : mixedLeft > 0 ? `Осталось разнести: ${formatSom(mixedLeft)}` : `Лишнее: ${formatSom(-mixedLeft)}`}
                  </span>
                  {mixedLeft > 0 && (
                    <span className="flex flex-wrap gap-1">
                      {SPLIT_METHODS.filter(payAllowed).map((m) => (
                        <button
                          key={m}
                          type="button"
                          className={touch ? tbtn : 'rounded border border-slate-300 bg-white px-2 py-0.5 text-xs hover:border-sky-500'}
                          onClick={() => setSplit({ ...split, [m]: somInput((parseSom(split[m]) ?? 0) + mixedLeft) })}
                        >
                          остаток → {PAYMENT_LABELS[m]}
                        </button>
                      ))}
                    </span>
                  )}
                </div>
              )}
            </div>
          )}
          {!touch && submitBlock}
        </Card>
        </div>
        {touch && <Card className="flex shrink-0 flex-col gap-2 p-2.5">{submitBlock}</Card>}
      </div>
      {lines.length > 0 && (
        <div className={`no-print fixed inset-x-0 z-20 ${touch ? 'bottom-0' : 'bottom-[52px]'} flex items-center gap-3 border-t border-slate-200 bg-white px-4 py-2 shadow-[0_-2px_8px_rgba(15,23,42,0.08)] md:hidden`}>
          <div className="min-w-0 flex-1">
            {blockers.length > 0 ? (
              <div className="truncate text-xs text-amber-700">Заполните: {blockers.map((x) => x.label).join(', ')}</div>
            ) : (
              <div className="text-xs text-slate-500">{bonusAmount > 0 ? 'К оплате (с баллами)' : batteryAmt > 0 ? 'К оплате (с аккумулятором)' : 'Итого'}</div>
            )}
            <div className="truncate text-xl font-bold">
              <Money value={due} />
            </div>
          </div>
          <Button className="min-h-12 shrink-0 px-6 py-3 text-base" disabled={!canSubmit} onClick={trySubmit}>
            {fix ? 'Сохранить' : 'Провести чек'}
          </Button>
        </div>
      )}
      {oilQuestion && (
        <Modal title="Это замена масла?" onClose={() => setOilQuestion(false)}>
          <div className="flex flex-col gap-4" data-testid="oil-question">
            <div className="text-base text-slate-700">
              В чеке моторное масло, а продажа — на вынос. Если масло меняют здесь, оформите замену: мастер получит ставку, а замена
              запишется в масляную книжку машины.
            </div>
            <div className="grid gap-2 sm:grid-cols-2">
              <Button
                className="min-h-14! text-lg!"
                onClick={() => {
                  setOilAsked(true)
                  setOilQuestion(false)
                  changeType('service')
                  toast('Замена масла: выберите вид замены, мастера, клиента и машину')
                }}
              >
                Да, замена
              </Button>
              <Button
                variant="secondary"
                className="min-h-14! text-lg!"
                onClick={() => {
                  setOilAsked(true)
                  setOilQuestion(false)
                  void submit()
                }}
              >
                Нет, на вынос
              </Button>
            </div>
          </div>
        </Modal>
      )}
      {shiftOpen && (
        <ShiftOpenModal
          owner={owner}
          cashiers={(employees.data ?? []).filter((e) => e.active && e.is_cashier)}
          till={shiftOpen.till}
          onClose={() => setShiftOpen(null)}
          onOpened={() => {
            setShiftOpen(null)
            shift.reload()
          }}
        />
      )}
      {clientOpen && (
        <Modal title="Клиент и машина" wide onClose={() => setClientOpen(false)}>
          <div className="flex flex-col gap-3">
            <div data-client-modal>
              <ClientPicker party={party} contactId={contactId} vehicleId={vehicleId} onParty={pickParty} onContact={setContactId} onVehicle={pickVehicle} />
            </div>
            {party && vehicleId && <OilHint vehicleId={vehicleId} />}
            <Button className="min-h-[64px]! text-xl!" onClick={() => setClientOpen(false)}>
              Готово
            </Button>
          </div>
        </Modal>
      )}
      {giftRule && <GiftPicker rule={giftRule} onPick={addGift} onClose={() => setGiftRule(null)} />}
      {expense && <QuickExpense onClose={() => setExpense(false)} />}
      {bonusOpen && (
        <BonusRedeem
          total={total}
          onClose={() => setBonusOpen(false)}
          initialPhone={party?.phone ?? ''}
          onApply={(b) => {
            setBonus(b)
            setBonusOpen(false)
            // Клиента ещё не выбрали — ставим того, чьи баллы.
            if (!party) void get<Party>(`/parties/${b.party_id}`).then(setParty).catch(() => undefined)
            toast(`Скидка баллами ${formatSom(b.amount_tyiyn)} добавлена в чек`)
          }}
        />
      )}
      {kindsOpen && <ServicePicker kinds ourOil={ourOil} onPick={addService} onClose={() => setKindsOpen(false)} />}
      {servicesOpen && (
        <ServicePicker
          ourOil={ourOil}
          onPick={addService}
          onClose={() => setServicesOpen(false)}
          onBatteryIntake={
            can(user, 'battery_intake')
              ? () => {
                  setServicesOpen(false)
                  setBatteryIntake(true)
                }
              : undefined
          }
          onBatterySale={() => {
            setServicesOpen(false)
            sellBatteries()
          }}
        />
      )}
      {debtPay && (
        <DebtorPayment
          party={party}
          onClose={() => setDebtPay(false)}
          // Оплатил клиент чека — его долг в чеке тоже свежий (SPEC-30 §13).
          onPaid={() => party && void get<Party>(`/parties/${party.id}`).then((p) => setParty((cur) => (cur?.id === p.id ? p : cur))).catch(() => undefined)}
        />
      )}
      {batteryIntake && <BatteryIntake onClose={() => setBatteryIntake(false)} />}
      {unknownCode !== null && (
        <UnknownCodeModal
          code={unknownCode}
          onClose={() => setUnknownCode(null)}
          onLinked={(p) => {
            setUnknownCode(null)
            addProduct(p)
            focusPicker()
          }}
          onCreateNew={() => {
            setNewProductCode(unknownCode)
            setUnknownCode(null)
          }}
        />
      )}
      {newProductName !== null && (
        <ProductFormModal
          presetName={newProductName}
          onClose={() => setNewProductName(null)}
          onSaved={(p) => {
            setNewProductName(null)
            addProduct(p)
          }}
        />
      )}
      {newProductCode !== null && (
        <ProductFormModal
          presetBarcode={newProductCode}
          onClose={() => setNewProductCode(null)}
          onSaved={(p) => {
            setNewProductCode(null)
            addProduct(p)
            focusPicker()
          }}
        />
      )}
    </div>
  )
}

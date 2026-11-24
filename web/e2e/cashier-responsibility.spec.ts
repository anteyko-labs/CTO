// Чек юрлица под ответственность кассира (задача 046, ADR-213): долг фирме в режиме «ждать» — фирма
// не отвечает, кассир пишет, почему проводит, чек проведён; владельцу — уведомление сразу.
import {
  expect,
  test,
  type APIRequestContext,
  type Page,
} from "@playwright/test";
import { ownerSafe } from "./safe";
import { openPay, ensureShift } from "./cashier";
import { signDebtAtTill } from "./sign";

const OWNER = process.env.E2E_LOGIN ?? "owner";
const OWNER_PASSWORD = process.env.E2E_PASSWORD ?? "owner-pass-1";
const DEVICE = { "X-Device-Id": "00000000-0000-4000-8000-00000000e046" };
const RUN = Date.now().toString().slice(-8);

async function login(page: Page) {
  // Печать документа о долге не открывает окно браузера.
  await page.addInitScript(() => {
    window.print = () => undefined;
  });
  await page.goto("/login");
  await page.getByLabel("Логин").fill(OWNER);
  await page.getByLabel("Пароль").fill(OWNER_PASSWORD);
  await page.getByRole("button", { name: "Войти" }).click();
  await expect(page.getByRole("link", { name: "Касса" })).toBeVisible();
}

async function api<T>(
  request: APIRequestContext,
  method: "GET" | "POST" | "PUT",
  path: string,
  data?: unknown,
): Promise<T> {
  const res = await request.fetch(`/api/v1${path}`, {
    method,
    data,
    headers: DEVICE,
  });
  expect(res.ok(), `${method} ${path}: ${await res.text()}`).toBeTruthy();
  return (await res.json()) as T;
}

test("долг фирме без её ответа — «Провести под мою ответственность»", async ({
  page,
}) => {
  await login(page);
  const request = page.request;
  await ensureShift(request);
  const code = `46${RUN}01`;
  const product = `Фильтр ответственности ${RUN}`;
  const cat = await api<{ id: string }>(request, "POST", "/categories", {
    name: `Кат 046 ${RUN}`,
    kind: "other",
  });
  const p = await api<{ id: string }>(request, "POST", "/products", {
    op_id: crypto.randomUUID(),
    category_id: cat.id,
    name: product,
    barcodes: [code],
    sale_price_tyiyn: 100_000,
  });
  await api(request, "POST", "/receipts", {
    op_id: crypto.randomUUID(),
    paid_from: await ownerSafe(request),
    lines: [{ product_id: p.id, qty: 3, cost_tyiyn: 3 * 50_000 }],
  });
  const firm = `ОсОО Молчун ${RUN}`;
  const party = await api<{ id: string }>(request, "POST", "/parties", {
    role: "customer",
    kind: "company",
    name: firm,
    phone: `0700${RUN.slice(-6)}`,
    inn: `046${RUN}123`,
  });
  // Кабинет есть — фирму спрашивают; режим «ждать» — без ответа чек не проходит.
  await api(request, "POST", `/parties/${party.id}/cabinet/reset`, {});
  const legal = await api<{
    approval_mode: string;
    approval_timeout_min: number;
  }>(request, "GET", "/settings/legal");
  await api(request, "PUT", "/settings/legal", {
    op_id: crypto.randomUUID(),
    approval_mode: "wait",
    approval_timeout_min: legal.approval_timeout_min,
  });
  try {
    await page.goto("/");
    const picker = page.getByPlaceholder(/Сканируйте штрихкод/);
    await picker.fill(code);
    await picker.press("Enter");
    await expect(page.getByText(product)).toBeVisible();
    await page.locator("[data-client-input]").fill(firm);
    await page.getByRole("button", { name: new RegExp(`^${firm}`) }).click();
    await openPay(page);
    await page
      .locator("[data-pay]")
      .getByRole("button", { name: "В долг", exact: true })
      .click();
    await page.getByRole("button", { name: "Провести чек" }).click();
    await signDebtAtTill(page);

    const wait = page.getByTestId("approval-wait");
    await expect(wait).toContainText("Ждём подтверждения фирмы");
    await wait
      .getByRole("button", { name: "Провести под мою ответственность" })
      .click();
    const box = page.getByTestId("responsibility");
    await expect(box).toContainText("Ждём подтверждения фирмы");
    // Без пояснения — не проводится.
    await box.getByRole("button", { name: "Провести", exact: true }).click();
    await expect(box).toContainText("Напишите, почему проводите");
    await box
      .locator("#responsibility-note")
      .fill("бухгалтер фирмы не берёт трубку");
    await box.getByRole("button", { name: "Провести", exact: true }).click();
    await expect(box).toHaveCount(0);

    // Чек есть, с меткой; владельцу — уведомление.
    await expect
      .poll(
        async () =>
          (
            await api<{
              sales: {
                party_name: string | null;
                responsibility?: string | null;
              }[];
            }>(request, "GET", "/sales")
          ).sales.find((s) => s.party_name === firm)?.responsibility ?? "",
      )
      .toContain("бухгалтер фирмы не берёт трубку");
    const notes = JSON.stringify(await api(request, "GET", "/notifications"));
    expect(notes).toContain("под ответственность кассира");
    expect(notes).toContain("бухгалтер фирмы не берёт трубку");
  } finally {
    await api(request, "PUT", "/settings/legal", {
      op_id: crypto.randomUUID(),
      approval_mode: legal.approval_mode,
      approval_timeout_min: legal.approval_timeout_min,
    });
  }
});

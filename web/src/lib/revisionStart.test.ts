import { describe, expect, it } from "vitest";
import { startMode } from "./revisionStart";

const head = (
  number: number,
  status: "draft" | "posted" | "cancelled",
  category_name: string | null = null,
) => ({
  id: `r${number}`,
  number,
  status,
  category_name,
});

describe("startMode (задача 019)", () => {
  it("владелец начинает всегда", () => {
    expect(startMode(true, [])).toEqual({ kind: "start" });
    expect(startMode(true, [head(1, "draft")])).toEqual({ kind: "start" });
  });

  it("администратор без открытой ревизии ждёт владельца", () => {
    expect(startMode(false, [])).toEqual({ kind: "wait" });
    expect(startMode(false, [head(1, "posted"), head(2, "cancelled")])).toEqual(
      { kind: "wait" },
    );
  });

  it("администратор присоединяется к начатой владельцем", () => {
    expect(
      startMode(false, [head(3, "draft", "Масла"), head(2, "posted")]),
    ).toEqual({
      kind: "join",
      drafts: [
        { id: "r3", number: 3, label: "Присоединиться к ревизии № 3 · Масла" },
      ],
    });
    expect(startMode(false, [head(4, "draft")])).toMatchObject({
      drafts: [{ label: "Присоединиться к ревизии № 4 · весь склад" }],
    });
  });
});

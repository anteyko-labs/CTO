import { describe, expect, it } from "vitest";
import { addReason, canTakeResponsibility, noteError } from "./responsibility";

describe("под ответственность кассира", () => {
  it("кнопка — только на отказы «нет ответа фирмы»", () => {
    expect(canTakeResponsibility("approval_required")).toBe(true);
    expect(canTakeResponsibility("contact_pending")).toBe(true);
    expect(canTakeResponsibility("vehicle_pending")).toBe(true);
    expect(canTakeResponsibility("approval_rejected")).toBe(false);
    expect(canTakeResponsibility("validation")).toBe(false);
    expect(canTakeResponsibility("")).toBe(false);
    expect(canTakeResponsibility(undefined)).toBe(false);
  });

  it("пояснение — от 5 до 300 знаков", () => {
    expect(noteError("")).toBe("Напишите, почему проводите");
    expect(noteError("    ")).toBe("Напишите, почему проводите");
    expect(noteError("нет")).not.toBeNull();
    expect(noteError(" звонил ")).toBeNull();
    expect(noteError("я".repeat(300))).toBeNull();
    expect(noteError("я".repeat(301))).not.toBeNull();
  });

  it("причины без повторов", () => {
    expect(
      addReason(
        addReason([], "Ждём подтверждения фирмы"),
        "Ждём подтверждения фирмы ",
      ),
    ).toEqual(["Ждём подтверждения фирмы"]);
    expect(addReason(["а"], "")).toEqual(["а"]);
  });
});

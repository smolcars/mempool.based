import { flush } from "solid-js";
import { describe, expect, it } from "vitest";
import { Exact } from "./Exact";
import { mount } from "./test-utils";

describe("Exact", () => {
  // As the network menu's (NetworkSwitch.test.tsx): Safari opened the disclosure, or followed the
  // link, under the tap that closed an exact amount.
  it("closes on a press outside that does nothing else, except on another figure", () => {
    const el = mount(() => (
      <>
        <Exact class="a" figure="1 BTC">
          <span class="num">1.00000000</span>
        </Exact>
        <Exact class="b" figure="2 BTC">
          <span class="num">2.00000000</span>
        </Exact>
      </>
    ));
    const [a, b] = [...el.querySelectorAll<HTMLButtonElement>("button[data-exact]")];
    if (!a || !b) throw new Error("no figures");
    const shown = () =>
      [...el.querySelectorAll<HTMLElement>(".exact")]
        .map((p) => (p.hidden === true ? 0 : 1))
        .join("");
    const under = document.createElement("button");
    let clicks = 0;
    let releases = 0;
    under.addEventListener("click", () => (clicks += 1));
    under.addEventListener("pointerup", () => (releases += 1));
    document.body.append(under);
    const press = (target: HTMLElement, end: "click" | "cancel" = "click") => {
      target.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true }));
      if (end === "cancel") {
        target.dispatchEvent(new PointerEvent("pointercancel", { bubbles: true }));
        return;
      }
      target.dispatchEvent(new PointerEvent("pointerup", { bubbles: true }));
      target.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, detail: 1 }));
      flush();
    };

    press(a);
    expect(shown()).toBe("10");
    // Outside: closed, and neither the release nor the click reached what was under it.
    press(under);
    expect(shown()).toBe("00");
    expect([clicks, releases]).toStrictEqual([0, 0]);
    // Nothing open: the next press goes where it lands.
    press(under);
    expect([clicks, releases]).toStrictEqual([1, 1]);
    // Another figure opens in the same press.
    press(a);
    press(b);
    expect(shown()).toBe("01");
    // A press that turns into a scroll arms nothing for the next one.
    press(under, "cancel");
    flush();
    expect(shown()).toBe("00");
    press(under);
    expect(clicks).toBe(2);
    // Escape closes it and gives the figure back its focus; a key's click always goes through.
    press(a);
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    flush();
    expect(shown()).toBe("00");
    expect(document.activeElement).toBe(a);
    under.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, detail: 0 }));
    expect(clicks).toBe(3);
    under.remove();
  });
});

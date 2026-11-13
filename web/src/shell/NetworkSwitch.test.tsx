import { flush } from "solid-js";
import { describe, expect, it } from "vitest";
import { mount } from "../components/test-utils";
import { NetworkSwitch } from "./NetworkSwitch";

describe("NetworkSwitch", () => {
  // In the page, not the browser's top layer: iOS Safari stopped the page above its toolbar while
  // a popover showed. It still closes on a press outside, on Escape and on a choice.
  it("opens its menu in the page and closes it on a press outside (doing nothing else), Escape or a choice", () => {
    const el = mount(() => <NetworkSwitch networks={() => ["mainnet", "signet"]} />, "/blocks");
    const button = el.querySelector<HTMLButtonElement>(".network__button");
    const menu = el.querySelector<HTMLElement>(".network__menu");
    if (!button || !menu) throw new Error("no menu");
    expect(menu.hasAttribute("popover")).toBe(false);
    const open = () => {
      button.click();
      flush();
      expect(menu.hidden).toBe(false);
      expect(button.getAttribute("aria-expanded")).toBe("true");
    };
    open();
    expect(menu.querySelector('a[data-network="signet"]')?.getAttribute("href")).toBe(
      "/signet/blocks",
    );
    // A tap outside only closes it: what is under the finger does nothing (Safari followed a link).
    const under = document.createElement("button");
    let clicks = 0;
    under.addEventListener("click", () => (clicks += 1));
    document.body.append(under);
    const tap = () => {
      under.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true }));
      under.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, detail: 1 }));
      return clicks;
    };
    /** A keyboard's activation: a click with no pointer behind it. */
    const enter = () => {
      under.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, detail: 0 }));
      return clicks;
    };
    expect(tap()).toBe(0);
    flush();
    expect(menu.hidden).toBe(true);
    expect(button.getAttribute("aria-expanded")).toBe("false");
    // With the menu closed, the next tap goes where it lands.
    expect(tap()).toBe(1);
    // A press that ends in no click (a right-click) never swallows a later key's activation.
    open();
    under.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true, button: 2 }));
    flush();
    expect(menu.hidden).toBe(true);
    expect(enter()).toBe(2);
    // Nor does a primary press whose click never came, once a key is pressed.
    open();
    under.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true }));
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Tab", bubbles: true }));
    expect(enter()).toBe(3);
    open();
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    flush();
    expect(menu.hidden).toBe(true);
    expect(document.activeElement).toBe(button);
    open();
    menu.querySelector<HTMLAnchorElement>('a[data-network="signet"]')?.click();
    flush();
    expect(menu.hidden).toBe(true);
  });
});

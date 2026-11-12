import { createSignal } from "solid-js";
import { describe, expect, it } from "vitest";
import { mount } from "../../components/test-utils";
import { Code } from "./Code";
import { PREVIEW_CHARS, SHOW_ALL_MAX } from "./tx";

const shown = (el: Element) => el.querySelector(".tx-code__text")?.textContent ?? "";
const button = (el: Element, name: string) =>
  [...el.querySelectorAll("button")].find((b) => b.textContent === name);

describe("Code", () => {
  it("previews, shows all on request, and offers copy only past the limit", () => {
    const el = mount(() => <Code label="Script" value={"ab".repeat(400)} />);
    expect(shown(el)).toHaveLength(PREVIEW_CHARS + 1);
    button(el, "Show all")?.click();
    expect(shown(el)).toHaveLength(800);

    const huge = mount(() => <Code label="Witness" value={"cd".repeat(SHOW_ALL_MAX)} />);
    expect(shown(huge)).toHaveLength(PREVIEW_CHARS + 1);
    expect(button(huge, "Show all")).toBeUndefined();
    expect(huge.textContent).toContain("Too long to show in full here. Copy it instead.");
  });

  it("collapses again when its value changes, so a huge new value is never laid out", () => {
    const [value, setValue] = createSignal("ab".repeat(400));
    const el = mount(() => <Code label="Script" value={value()} />);
    button(el, "Show all")?.click();
    expect(shown(el)).toHaveLength(800);
    setValue("ef".repeat(SHOW_ALL_MAX));
    expect(shown(el)).toHaveLength(PREVIEW_CHARS + 1);
    setValue("ab".repeat(400));
    expect(shown(el)).toHaveLength(PREVIEW_CHARS + 1);
  });
});

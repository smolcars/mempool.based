import { flush } from "solid-js";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { CopyButton } from "./CopyButton";
import { mount } from "./test-utils";

const VALUE = "4dd40e87c671742730397cc615137b5554c8a2e5ec32df02daf5ba3d7a507e4c";

/** What the page has: the async clipboard (a secure context) or none (plain HTTP). */
function clipboard(writeText: ((text: string) => Promise<void>) | undefined) {
  Object.defineProperty(navigator, "clipboard", {
    configurable: true,
    value: writeText ? { writeText } : undefined,
  });
}

/** The copy command: answers `result`, and notes what was selected when it ran. */
function command(result: boolean | "throws") {
  const seen: { selected: string; inPage: boolean }[] = [];
  const exec = vi.fn<(name: string) => boolean>((name) => {
    expect(name).toBe("copy");
    const area = document.querySelector("textarea.copy__scratch");
    if (area instanceof HTMLTextAreaElement) {
      seen.push({
        selected: area.value.slice(area.selectionStart, area.selectionEnd),
        inPage: area.isConnected && area.readOnly,
      });
    }
    if (result === "throws") throw new Error("not allowed");
    return result;
  });
  Object.defineProperty(document, "execCommand", { configurable: true, value: exec });
  return { exec, seen };
}

/** Presses the copy button and waits for its answer. */
async function press() {
  const el = mount(() => <CopyButton value={VALUE} label="Copy transaction ID" />);
  const button = el.querySelector("button");
  if (!button) throw new Error("no button");
  button.focus();
  button.click();
  await vi.waitFor(() => {
    flush();
    expect(button.textContent).not.toContain("Copy transaction ID");
  });
  return button;
}

beforeEach(() => {
  vi.useRealTimers();
});
afterEach(() => {
  Reflect.deleteProperty(navigator, "clipboard");
  Reflect.deleteProperty(document, "execCommand");
});

describe("CopyButton", () => {
  it("copies through the async clipboard where the page has it", async () => {
    const writeText = vi.fn<(text: string) => Promise<void>>(async () => {});
    clipboard(writeText);
    const { exec } = command(true);
    const button = await press();
    expect(writeText).toHaveBeenCalledWith(VALUE);
    expect(exec).not.toHaveBeenCalled();
    expect(button.textContent).toContain("Copied");
    expect(button.dataset["copied"]).toBe("");
  });

  it("copies with the copy command where there is no async clipboard (plain HTTP)", async () => {
    clipboard(undefined);
    const { exec, seen } = command(true);
    const button = await press();
    expect(exec).toHaveBeenCalledOnce();
    // The whole value, selected in a read-only field that was in the page while it copied.
    expect(seen).toStrictEqual([{ selected: VALUE, inPage: true }]);
    expect(document.querySelector("textarea")).toBeNull();
    expect(document.activeElement).toBe(button);
    expect(button.textContent).toContain("Copied");
  });

  it("falls back to the copy command when the async clipboard refuses", async () => {
    clipboard(() => Promise.reject(new DOMException("denied", "NotAllowedError")));
    const { exec } = command(true);
    const button = await press();
    expect(exec).toHaveBeenCalledOnce();
    expect(button.textContent).toContain("Copied");
  });

  it.each([false, "throws"] as const)(
    "says it could not copy when both ways fail (command answers %s)",
    async (result) => {
      clipboard(() => Promise.reject(new DOMException("denied", "NotAllowedError")));
      command(result);
      const button = await press();
      expect(button.textContent).not.toContain("Copied");
      expect(button.textContent).toContain("Could not copy");
      expect(button.textContent).toContain("Not copied");
      expect(button.dataset["failed"]).toBe("");
      expect(button.dataset["copied"]).toBeUndefined();
      expect(document.querySelector("textarea")).toBeNull();
    },
  );

  it("never says Copied without a copy, even with no clipboard at all", async () => {
    clipboard(undefined);
    command(false);
    const button = await press();
    expect(button.textContent).not.toContain("Copied");
    expect(button.querySelector("[aria-live]")?.textContent).toBe("Could not copy");
  });
});

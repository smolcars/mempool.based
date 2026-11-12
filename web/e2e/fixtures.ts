// Every browser test imports `test` and `expect` from here, never from @playwright/test.
import { test as base, expect } from "@playwright/test";

export { expect };

export const test = base.extend<{ consoleErrorsAllowed: RegExp[]; noConsoleErrors: undefined }>({
  // Console errors a test causes on purpose, such as a fetch that fails. CSP violations never pass.
  consoleErrorsAllowed: [[], { option: true }],
  // Runs around every test: a console error, an uncaught exception or a CSP violation fails it.
  noConsoleErrors: [
    async ({ page, consoleErrorsAllowed }, use) => {
      const errors: string[] = [];
      page.on("console", (m) => {
        if (m.type() === "error") errors.push(m.text());
      });
      page.on("pageerror", (e) => errors.push(`uncaught: ${e.message}`));
      await use(undefined);
      const unexpected = errors.filter(
        (e) => /Content Security Policy/i.test(e) || !consoleErrorsAllowed.some((r) => r.test(e)),
      );
      expect(unexpected, "console errors and CSP violations").toEqual([]);
    },
    { auto: true },
  ],
});

/** The chain `dev/regtest/regtest.py scenario` builds; every hash is fixed (docs/REGTEST.md). */
export const chain = {
  tip: { height: 110, hash: "3fa932c75119e4a4f4c4f5f43afe8fec03c5f3bc9b84fbadd1e96dd34bc6654d" },
  blocks: {
    1: "55c72c83b6934bee3a62540b5bc7f0438e9dcefb03779b87e8456bc545171960",
    106: "2ac64444e3172ee94aeeff3b09014b1d69b4daeccddac025d493332d54f6f145",
  },
  txs: {
    script_types: "44c64894ca2125fb361f286005f8e27d1c1846bfe00c692925ab4d6844721bbd",
    consolidation: "f8580c086f549b53145643e6b846d617aef17305ec2af34125e41faa74bfd787",
    spend_p2sh_p2a: "aa4652e9b6189d3569c54d148016f38538fea374c22965dbbe472ffef5c25f5f",
    payment: "0def111d15485da613e9dc24a587a9657d3726772dcc68e0c55c2daa67b1dca9",
    rbf_original: "c465329ca476d6454a42d21372a3461c2753426a8a07c46e0f2a5d6a731b756d",
    rbf_replacement: "39134afd670713636c838ddb195e1c9d45cd2625b01d19fc73f4cae5ba51a9f8",
    cpfp_parent: "7621473f23691b3acc681cada96b25a64e64ff298d8f95fd4818855ea886abbd",
    cpfp_child: "ad853f8771d8e8f764d362723b23fe758654432f52da63baa993ffc66af93f51",
  },
  addresses: {
    p2pkh: "mwLW3XDqcAYmYKp8VDEYXweSBJ5mTUAuat",
    p2sh: "2ND8PB9RrfCaAcjfjP1Y6nAgFd9zWHYX4DN",
    p2wpkh: "bcrt1qw5puevk8wgy5t9d2qjg6frpfmesuqnnpx95u7s",
    p2wsh: "bcrt1qgpsh04whvyp5fvrul6rr8384qt0mf4a7h507m5ed79d72euknpgqpmza40",
    p2tr: "bcrt1pz9ml8p3h96z9hlvz0esm3uwdq9pxssmykz209jrg57znrdvx5y0shy3atk",
    p2a: "bcrt1pfeesnyr2tx",
    miner: "bcrt1qft5p2uhsdcdc3l2ua4ap5qqfg4pjaqlp250x7us7a8qqhrxrxfsqseac85",
  },
} as const;

/**
 * Lightning funding via Atomiq (FROM_BTCLN_AUTO → strkBTC).
 *
 * This module is Fund-screen only. Betting never imports it — place_bet is
 * 100% hidden Starknet calls with no LN invoice, zap, or Atomiq swap.
 *
 * Atomiq packages ship `/// <reference types="node" />` in their .d.ts files.
 * A static import would pull Node timer globals into the whole desktop
 * typecheck (breaking DOM `setTimeout` typing in unrelated files). Load them
 * through an opaque dynamic import so tsc cannot resolve those refs.
 */

import { LN_MAX_SATS, LN_MIN_SATS } from "./constants";

/** Opaque module loader — intentionally unresolvable to package .d.ts. */
async function importAtomiq(): Promise<{
  // Minimal structural surface we need; keep loose to avoid Node globals.
  BitcoinNetwork: { MAINNET: unknown };
  SwapAmountType: { EXACT_IN: unknown };
  SwapperFactory: new (
    initializers: unknown[],
  ) => {
    Tokens: {
      BITCOIN: { BTCLN: unknown };
      STARKNET: { strkBTC: unknown };
    };
    newSwapper: (cfg: unknown) => PromiseLike<{
      init: () => Promise<void>;
      swap: (...args: unknown[]) => Promise<AtomiqSwap>;
    }> & {
      init: () => Promise<void>;
      swap: (...args: unknown[]) => Promise<AtomiqSwap>;
    };
  };
  StarknetInitializer: unknown;
}> {
  const dynamicImport = new Function("m", "return import(m)") as (
    m: string,
  ) => Promise<Record<string, unknown>>;
  const [sdk, chain] = await Promise.all([
    dynamicImport("@atomiqlabs/sdk"),
    dynamicImport("@atomiqlabs/chain-starknet"),
  ]);
  return {
    BitcoinNetwork: sdk.BitcoinNetwork as { MAINNET: unknown },
    SwapAmountType: sdk.SwapAmountType as { EXACT_IN: unknown },
    SwapperFactory: sdk.SwapperFactory as never,
    StarknetInitializer: chain.StarknetInitializer,
  };
}

type AtomiqSwap = {
  getAddress: () => string;
  getHyperlink: () => string;
  getOutput: () => { rawAmount: string | number | bigint };
  getQuoteExpiry: () => number;
  execute: (
    wallet: { payInvoice: () => Promise<string> },
    opts: Record<string, unknown>,
  ) => Promise<unknown>;
};

export type FundLightningQuote = {
  invoice: string;
  hyperlink: string;
  /** Human-facing input amount description. */
  inputSats: bigint;
  /** Output amount in 8dp raw units (product label: BTC). */
  outputRaw: bigint;
  expiryMs: number;
  /** Underlying swap handle for execute/wait. */
  swap: AtomiqSwap;
};

export type FundLightningOptions = {
  amountSats: bigint;
  /** Counterfactual NostrAccount address (destination). */
  destinationAddress: string;
  starknetRpcUrl: string;
  /**
   * Optional gas drop in STRK wei. Fallback only — AVNU sponsors gas for bets.
   * Omit in the happy path.
   */
  gasAmount?: bigint;
};

function assertSatsInRange(amountSats: bigint) {
  if (amountSats < LN_MIN_SATS || amountSats > LN_MAX_SATS) {
    throw new Error(
      `Amount must be between ${LN_MIN_SATS} and ${LN_MAX_SATS} sats`,
    );
  }
}

/**
 * Create a live Lightning → BTC (strkBTC) quote into the hidden wallet.
 * Does not place a bet.
 */
export async function createFundLightningQuote(
  options: FundLightningOptions,
): Promise<FundLightningQuote> {
  assertSatsInRange(options.amountSats);

  const atomiq = await importAtomiq();
  const Factory = new atomiq.SwapperFactory([
    atomiq.StarknetInitializer,
  ] as never[]);
  const Tokens = Factory.Tokens;

  const swapper = Factory.newSwapper({
    chains: {
      STARKNET: {
        rpcUrl: options.starknetRpcUrl,
      },
    },
    bitcoinNetwork: atomiq.BitcoinNetwork.MAINNET,
  });
  await swapper.init();

  const swapOpts =
    options.gasAmount !== undefined ? { gasAmount: options.gasAmount } : {};

  const swap = await swapper.swap(
    Tokens.BITCOIN.BTCLN,
    Tokens.STARKNET.strkBTC,
    options.amountSats,
    atomiq.SwapAmountType.EXACT_IN,
    undefined,
    options.destinationAddress,
    swapOpts,
  );

  return {
    invoice: swap.getAddress(),
    hyperlink: swap.getHyperlink(),
    inputSats: options.amountSats,
    outputRaw: BigInt(swap.getOutput().rawAmount),
    expiryMs: swap.getQuoteExpiry(),
    swap,
  };
}

/**
 * Wait for the Lightning payment and automatic settlement.
 * Fund path only — never call from place_bet.
 */
export async function waitFundLightningSettlement(
  swap: AtomiqSwap,
): Promise<{ automatic: boolean; claimTxId?: string }> {
  const automatic = await swap.execute(
    {
      // External wallet pays the invoice shown in the Fund UI.
      payInvoice: async () => "",
    },
    {},
  );
  return { automatic: Boolean(automatic) };
}

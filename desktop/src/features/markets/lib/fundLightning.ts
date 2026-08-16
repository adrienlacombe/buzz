/**
 * Lightning funding via Atomiq (FROM_BTCLN_AUTO → strkBTC).
 *
 * This module is Fund-screen only. Betting never imports it — place_bet is
 * 100% hidden Starknet calls with no LN invoice, zap, or Atomiq swap.
 */

import {
  BitcoinNetwork,
  SwapAmountType,
  SwapperFactory,
} from "@atomiqlabs/sdk";
import { StarknetInitializer } from "@atomiqlabs/chain-starknet";

import { LN_MAX_SATS, LN_MIN_SATS } from "./constants";

const Factory = new SwapperFactory([StarknetInitializer] as const);
const Tokens = Factory.Tokens;

export type FundLightningQuote = {
  invoice: string;
  hyperlink: string;
  /** Human-facing input amount description. */
  inputSats: bigint;
  /** Output amount in 8dp raw units (product label: BTC). */
  outputRaw: bigint;
  expiryMs: number;
  /** Underlying swap handle for execute/wait. */
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  swap: any;
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

  const swapper = Factory.newSwapper({
    chains: {
      STARKNET: {
        rpcUrl: options.starknetRpcUrl,
      },
    },
    bitcoinNetwork: BitcoinNetwork.MAINNET,
  });
  await swapper.init();

  const swapOpts =
    options.gasAmount !== undefined ? { gasAmount: options.gasAmount } : {};

  const swap = await swapper.swap(
    Tokens.BITCOIN.BTCLN,
    Tokens.STARKNET.strkBTC,
    options.amountSats,
    SwapAmountType.EXACT_IN,
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
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  swap: any,
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

import * as React from "react";
import { QRCodeSVG } from "qrcode.react";
import { toast } from "sonner";

import { invokeTauri } from "@/shared/api/tauri";
import { Button } from "@/shared/ui/button";
import { Input } from "@/shared/ui/input";

import {
  DIFFICULTY_MARKET,
  LN_MAX_SATS,
  LN_MIN_SATS,
  MARKET_TITLE,
  MIN_TRADE_RAW,
} from "./lib/constants";
import { createFundLightningQuote } from "./lib/fundLightning";
import { bettingHalted } from "./lib/halt";
import {
  fetchIndexerHealth,
  fetchIndexerMarkets,
  findDifficultyMarket,
  resolveIndexerUrl,
  type IndexerMarket,
} from "./lib/indexer";
import { placeBet } from "./lib/placeBet";

type Tab = "bet" | "fund";

async function fetchBitcoinHeight(): Promise<number> {
  const res = await fetch("https://mempool.space/api/blocks/tip/height");
  if (!res.ok) {
    throw new Error("Could not read Bitcoin tip height");
  }
  return Number(await res.text());
}

export function MarketsScreen() {
  const [tab, setTab] = React.useState<Tab>("bet");
  const [market, setMarket] = React.useState<IndexerMarket | null>(null);
  const [height, setHeight] = React.useState<number | null>(null);
  const [halted, setHalted] = React.useState(false);
  const [targetDifficulty, setTargetDifficulty] = React.useState("");
  const [collateralBtc, setCollateralBtc] = React.useState("0.001");
  const [busy, setBusy] = React.useState(false);
  const [fundSats, setFundSats] = React.useState("10000");
  const [invoice, setInvoice] = React.useState<string | null>(null);
  const [walletAddress, setWalletAddress] = React.useState<string | null>(null);
  const [loadError, setLoadError] = React.useState<string | null>(null);
  const [indexerHost, setIndexerHost] = React.useState<string | null>(null);

  React.useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const base = resolveIndexerUrl();
        await fetchIndexerHealth(base);
        const markets = await fetchIndexerMarkets(base);
        // Indexer returns unpadded address (0x23b3…); match felt-normalized.
        const found = findDifficultyMarket(markets, DIFFICULTY_MARKET);
        const tip = await fetchBitcoinHeight();
        if (cancelled) return;
        setIndexerHost(base);
        setMarket(found);
        setHeight(tip);
        setHalted(bettingHalted(tip));
        if (found?.state?.mean != null) {
          // Indexer mean for lognormal is μ = ln(D); show D on the axis.
          const d = Math.exp(found.state.mean);
          if (Number.isFinite(d) && d > 0) {
            setTargetDifficulty(d.toExponential(4));
          }
        }
      } catch (e) {
        if (!cancelled) {
          setLoadError(e instanceof Error ? e.message : String(e));
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  const onPlaceBet = React.useCallback(async () => {
    if (!market?.state || height == null) {
      toast.error("Market not ready");
      return;
    }
    if (halted) {
      toast.error("Betting is paused until after the next difficulty retarget");
      return;
    }
    const rawDifficulty = Number(targetDifficulty);
    if (!(rawDifficulty > 0)) {
      toast.error("Enter a positive target difficulty");
      return;
    }
    const collateral = Number(collateralBtc);
    if (!(collateral > 0)) {
      toast.error("Enter a BTC amount");
      return;
    }
    const raw = BigInt(Math.ceil(collateral * 1e8));
    if (raw < MIN_TRADE_RAW) {
      toast.error(
        `Minimum is ${(Number(MIN_TRADE_RAW) / 1e8).toFixed(6)} BTC`,
      );
      return;
    }

    setBusy(true);
    try {
      const result = await placeBet({
        rawDifficulty,
        bitcoinHeight: height,
        market: {
          mu: market.state.mean ?? 0,
          variance: market.state.variance ?? 0.01,
          sigma: market.state.sigma ?? Math.sqrt(market.state.variance ?? 0.01),
          effectiveK: market.state.effectiveK ?? market.state.k ?? 1,
        },
      });
      toast.success(`Bet placed · ${result.summary}`);
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }, [collateralBtc, halted, height, market, targetDifficulty]);

  const onFund = React.useCallback(async () => {
    const sats = BigInt(fundSats || "0");
    if (sats < LN_MIN_SATS || sats > LN_MAX_SATS) {
      toast.error(`Amount must be ${LN_MIN_SATS}–${LN_MAX_SATS} sats`);
      return;
    }
    setBusy(true);
    setInvoice(null);
    try {
      // Rust: human address only. Atomiq runs in JS on the Fund screen.
      const wallet = await invokeTauri<{
        address: string;
      }>("fund_lightning", { amountSats: Number(sats) });
      setWalletAddress(wallet.address);

      const rpc =
        import.meta.env.VITE_STARKNET_RPC_URL ??
        "https://mainnet.nodes.starknet.org/rpc/v0_10";
      const quote = await createFundLightningQuote({
        amountSats: sats,
        destinationAddress: wallet.address,
        starknetRpcUrl: rpc,
      });
      setInvoice(quote.invoice);
      toast.success("Lightning invoice ready — pay to add bitcoin");
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }, [fundSats]);

  return (
    <div className="flex h-full flex-col gap-4 overflow-auto p-6">
      <header className="space-y-1">
        <h1 className="text-xl font-semibold tracking-tight">Markets</h1>
        <p className="text-muted-foreground text-sm">{MARKET_TITLE}</p>
        <p className="text-muted-foreground text-xs">
          {indexerHost ? `Indexer ${indexerHost}` : "Indexer unset (INDEXER_URL required)"}
          {height != null ? ` · Bitcoin tip ${height}` : null}
          {halted ? " · betting paused near retarget" : null}
        </p>
      </header>

      {loadError ? (
        <div className="border-destructive/40 bg-destructive/10 rounded-md border p-3 text-sm">
          {loadError}
        </div>
      ) : null}

      <div className="flex gap-2">
        <Button
          variant={tab === "bet" ? "default" : "outline"}
          onClick={() => setTab("bet")}
        >
          Place bet
        </Button>
        <Button
          variant={tab === "fund" ? "default" : "outline"}
          onClick={() => setTab("fund")}
        >
          Add bitcoin
        </Button>
      </div>

      {tab === "bet" ? (
        <section className="max-w-lg space-y-4 rounded-lg border p-4">
          <p className="text-sm">
            Pick a target mean on the raw Bitcoin difficulty axis. Collateral is
            BTC. Betting pauses 24 blocks before each difficulty retarget.
          </p>
          <label className="block space-y-1 text-sm">
            <span>Target difficulty (D)</span>
            <Input
              value={targetDifficulty}
              onChange={(e) => setTargetDifficulty(e.target.value)}
              placeholder="e.g. 1.1e14"
              disabled={halted || busy}
            />
          </label>
          <label className="block space-y-1 text-sm">
            <span>Collateral (BTC)</span>
            <Input
              value={collateralBtc}
              onChange={(e) => setCollateralBtc(e.target.value)}
              disabled={halted || busy}
            />
          </label>
          <Button onClick={() => void onPlaceBet()} disabled={halted || busy}>
            {busy ? "Placing…" : halted ? "Paused until retarget" : "Place bet"}
          </Button>
        </section>
      ) : (
        <section className="max-w-lg space-y-4 rounded-lg border p-4">
          <p className="text-sm">
            Fund with Lightning. Pays into your wallet as BTC. This screen never
            places a bet.
          </p>
          <label className="block space-y-1 text-sm">
            <span>Amount (sats)</span>
            <Input
              value={fundSats}
              onChange={(e) => setFundSats(e.target.value)}
              disabled={busy}
            />
          </label>
          <Button onClick={() => void onFund()} disabled={busy}>
            {busy ? "Creating invoice…" : "Fund with Lightning"}
          </Button>
          {walletAddress ? (
            <p className="text-muted-foreground break-all text-xs">
              Destination ready
            </p>
          ) : null}
          {invoice ? (
            <div className="flex flex-col items-center gap-3">
              <QRCodeSVG value={invoice.toUpperCase()} size={180} />
              <p className="break-all text-xs">{invoice}</p>
            </div>
          ) : null}
        </section>
      )}
    </div>
  );
}

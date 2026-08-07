import { describe, expect, it } from "vitest";
import {
  costBasisNote,
  sessionSpend,
  spendQualifier,
  formatActual,
  formatCost,
  formatUsd,
  hasPrice,
  totalSpend,
  UNKNOWN_COST_LABEL,
} from "../cost";

describe("formatUsd", () => {
  it("uses two decimals at and above a dime", () => {
    expect(formatUsd(4.2)).toBe("$4.20");
    expect(formatUsd(12.5)).toBe("$12.50");
    expect(formatUsd(0.1)).toBe("$0.10");
  });

  it("keeps sub-dime prices from rounding away", () => {
    expect(formatUsd(0.043)).toBe("$0.043");
    expect(formatUsd(0.0035)).toBe("$0.0035");
  });

  it("prints an explicit zero as a price, not as free", () => {
    expect(formatUsd(0)).toBe("$0.00");
  });

  it("treats a non-price number as unknown rather than showing it", () => {
    expect(formatUsd(Number.NaN)).toBe(UNKNOWN_COST_LABEL);
    expect(formatUsd(Number.POSITIVE_INFINITY)).toBe(UNKNOWN_COST_LABEL);
    expect(formatUsd(-1)).toBe(UNKNOWN_COST_LABEL);
  });
});

describe("formatCost", () => {
  it("renders a null estimate as unavailable, never as zero", () => {
    expect(formatCost(null)).toBe(UNKNOWN_COST_LABEL);
    expect(formatCost(undefined)).toBe(UNKNOWN_COST_LABEL);
    expect(formatCost(null)).not.toContain("0.00");
  });

  it("renders a real estimate", () => {
    expect(formatCost({ usd: 2.43, basis: "8s at 720p" })).toBe("$2.43");
  });

  it("distinguishes unknown from priced", () => {
    expect(hasPrice(null)).toBe(false);
    expect(hasPrice({ usd: 0, basis: "local" })).toBe(true);
  });
});

describe("costBasisNote", () => {
  it("explains the missing price", () => {
    expect(costBasisNote(null)).toBe("No published price for this route");
  });

  it("calls out a provider floor so the number is not read as a bug", () => {
    expect(
      costBasisNote({ usd: 0.35, basis: "4s at 720p", minimumApplied: true }),
    ).toBe("4s at 720p · provider minimum applied");
  });

  it("passes the basis through untouched otherwise", () => {
    expect(costBasisNote({ usd: 1, basis: "1 image at 2K" })).toBe(
      "1 image at 2K",
    );
  });
});

describe("formatActual", () => {
  it("separates 'not reported' from a reported zero", () => {
    expect(formatActual(undefined)).toBe("Not reported");
    expect(formatActual(null)).toBe("Not reported");
    expect(formatActual(0)).toBe("$0.00");
  });
});

describe("totalSpend", () => {
  it("counts unknown prices instead of folding them in as zero", () => {
    const { usd, unknownCount } = totalSpend([1.5, null, 0.25, undefined]);
    expect(usd).toBeCloseTo(1.75);
    expect(unknownCount).toBe(2);
  });

  it("returns a zero total with no entries", () => {
    expect(totalSpend([])).toEqual({ usd: 0, unknownCount: 0 });
  });
});

describe("sessionSpend", () => {
  // The regression this exists for: eighteen paid generations, each carrying a
  // real estimate, displayed as "$0.00 · 18 unpriced". The wire mapper writes
  // `null` for a missing actual cost and the meter tested `=== undefined`, so
  // every job fell through to unknown.
  it("counts the estimate when the provider reported no charge", () => {
    const spend = sessionSpend([
      { actualUsd: null, estimatedUsd: 0.2 },
      { actualUsd: null, estimatedUsd: 0.2 },
    ]);
    expect(spend.usd).toBeCloseTo(0.4);
    expect(spend.estimatedCount).toBe(2);
    expect(spend.unknownCount).toBe(0);
  });

  it("treats an absent field the same as an explicit null", () => {
    // Rust's `Option<f64>` is `null` on the wire; a hand-built object in a test
    // or a recipe import is `undefined`. Both mean the same thing.
    expect(sessionSpend([{ estimatedUsd: 0.2 }]).usd).toBeCloseTo(0.2);
  });

  it("prefers a reported charge over our own guess", () => {
    const spend = sessionSpend([{ actualUsd: 0.31, estimatedUsd: 0.2 }]);
    expect(spend.usd).toBeCloseTo(0.31);
    expect(spend.actualCount).toBe(1);
    expect(spend.estimatedCount).toBe(0);
  });

  it("never reads an unknown price as free", () => {
    const spend = sessionSpend([
      { actualUsd: null, estimatedUsd: null },
      { actualUsd: null, estimatedUsd: 0.2 },
    ]);
    expect(spend.usd).toBeCloseTo(0.2);
    expect(spend.unknownCount).toBe(1);
  });

  it("counts a genuine zero as measured, not as missing", () => {
    // A local ComfyUI route really does cost nothing, and that is a fact worth
    // reporting rather than an absence of one.
    const spend = sessionSpend([{ actualUsd: 0 }]);
    expect(spend.actualCount).toBe(1);
    expect(spend.unknownCount).toBe(0);
  });

  it("says which part of the total is a guess", () => {
    expect(spendQualifier({ estimatedCount: 18, unknownCount: 0 })).toBe(
      "18 estimated",
    );
    expect(spendQualifier({ estimatedCount: 2, unknownCount: 3 })).toBe(
      "2 estimated · 3 unpriced",
    );
    expect(spendQualifier({ estimatedCount: 0, unknownCount: 0 })).toBeNull();
  });
});

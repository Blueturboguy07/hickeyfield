import { describe, expect, it } from "vitest";
import {
  costBasisNote,
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

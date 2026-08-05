import { describe, expect, it } from "vitest";
import { parseEnvText, planEnvImport } from "../env-import";
import type { ProviderInfo } from "../providers";

const valueOf = (text: string, name: string) =>
  parseEnvText(text).entries.find((e) => e.name === name)?.value;

describe("parseEnvText", () => {
  it("reads a plain assignment", () => {
    expect(parseEnvText("FAL_KEY=abc123").entries).toEqual([
      { name: "FAL_KEY", value: "abc123" },
    ]);
  });

  it("accepts the export prefix people copy out of a shell profile", () => {
    expect(valueOf("export FAL_KEY=abc123", "FAL_KEY")).toBe("abc123");
    expect(valueOf("export   FAL_KEY=abc123", "FAL_KEY")).toBe("abc123");
  });

  it("strips matching quotes without touching the contents", () => {
    expect(valueOf('FAL_KEY="abc 123"', "FAL_KEY")).toBe("abc 123");
    expect(valueOf("FAL_KEY='abc 123'", "FAL_KEY")).toBe("abc 123");
    // A quote character inside a single-quoted value is literal.
    expect(valueOf(`FAL_KEY='ab"cd'`, "FAL_KEY")).toBe('ab"cd');
  });

  it("keeps a hash that is part of the key", () => {
    // Only whitespace-then-# opens a comment; a key containing # is not one.
    expect(valueOf("FAL_KEY=ab#cd", "FAL_KEY")).toBe("ab#cd");
    expect(valueOf("FAL_KEY=abcd # the good one", "FAL_KEY")).toBe("abcd");
    expect(valueOf('FAL_KEY="ab cd" # note', "FAL_KEY")).toBe("ab cd");
  });

  it("ignores comments and blank lines entirely", () => {
    const { entries, unparsed } = parseEnvText(
      "# providers\n\n   \nFAL_KEY=a\n\n#OPENAI_API_KEY=b\n",
    );
    expect(entries).toEqual([{ name: "FAL_KEY", value: "a" }]);
    expect(unparsed).toEqual([]);
  });

  it("handles CRLF and lone CR line endings", () => {
    expect(parseEnvText("FAL_KEY=a\r\nOPENAI_API_KEY=b\r\n").entries).toEqual([
      { name: "FAL_KEY", value: "a" },
      { name: "OPENAI_API_KEY", value: "b" },
    ]);
    expect(valueOf("FAL_KEY=a\r", "FAL_KEY")).toBe("a");
  });

  it("reports lines it cannot read rather than dropping them", () => {
    const { entries, unparsed } = parseEnvText(
      "just some prose\n=novalue\n9BAD=x\nFAL_KEY=a",
    );
    expect(entries).toEqual([{ name: "FAL_KEY", value: "a" }]);
    expect(unparsed).toEqual(["just some prose", "=novalue", "9BAD=x"]);
  });

  it("unescapes only inside double quotes", () => {
    expect(valueOf('K="a\\nb"', "K")).toBe("a\nb");
    expect(valueOf("K='a\\nb'", "K")).toBe("a\\nb");
  });

  it("keeps an = that appears inside the value", () => {
    expect(valueOf("FAL_KEY=abc=def==", "FAL_KEY")).toBe("abc=def==");
  });
});

describe("planEnvImport", () => {
  it("maps conventional names onto providers", () => {
    const plan = planEnvImport(
      [
        "FAL_KEY=f1",
        "OPENAI_API_KEY=o1",
        "GEMINI_API_KEY=g1",
        "export XAI_API_KEY=x1",
      ].join("\n"),
    );
    expect(plan.providers).toEqual(["fal", "openai", "google", "xai"]);
    expect(plan.unknown).toEqual([]);
  });

  it("is case-insensitive about the variable name", () => {
    expect(planEnvImport("fal_key=f1").providers).toEqual(["fal"]);
  });

  it("routes the two halves of a key/secret provider separately", () => {
    const plan = planEnvImport(
      "HIGGSFIELD_API_KEY=k\nHIGGSFIELD_SECRET=s",
    );
    expect(plan.providers).toEqual(["higgsfield"]);
    expect(plan.assignments).toEqual([
      { provider: "higgsfield", secretHalf: false, value: "k" },
      { provider: "higgsfield", secretHalf: true, value: "s" },
    ]);
  });

  it("understands the app's own HALATION_ dev overrides", () => {
    const plan = planEnvImport("HALATION_FAL_KEY=f\nHALATION_HIGGSFIELD_SECRET=s");
    expect(plan.assignments).toEqual([
      { provider: "fal", secretHalf: false, value: "f" },
      { provider: "higgsfield", secretHalf: true, value: "s" },
    ]);
  });

  it("reports unrecognised variables instead of silently skipping them", () => {
    const plan = planEnvImport("DATABASE_URL=postgres://x\nFAL_KEY=f1");
    expect(plan.providers).toEqual(["fal"]);
    expect(plan.unknown).toEqual(["DATABASE_URL=…"]);
  });

  it("never echoes the value of an unrecognised variable", () => {
    // The pasted block is full of live credentials; an "unknown line" readout
    // that repeated them would put them back on screen.
    const plan = planEnvImport("STRIPE_SECRET_KEY=sk_live_supersecret");
    expect(plan.unknown.join(" ")).not.toContain("sk_live_supersecret");
  });

  it("refuses to import a blank value, because blank clears the keychain", () => {
    const plan = planEnvImport("FAL_KEY=\nOPENAI_API_KEY=o1");
    expect(plan.providers).toEqual(["openai"]);
    expect(plan.assignments).toHaveLength(1);
    expect(plan.unknown).toEqual(["FAL_KEY= (no value)"]);
  });

  it("lets the last occurrence of a name win", () => {
    const plan = planEnvImport("FAL_KEY=old\nFAL_KEY=new");
    expect(plan.assignments).toEqual([
      { provider: "fal", secretHalf: false, value: "new" },
    ]);
  });

  it("accepts a catalogue supplied at runtime by provider_info", () => {
    const catalog: ProviderInfo[] = [
      {
        slug: "newthing",
        displayName: "New Thing",
        needsKey: true,
        needsSecret: false,
        keyUrl: "",
        envNames: ["NEWTHING_TOKEN"],
        blurb: "",
      },
    ];
    const plan = planEnvImport("NEWTHING_TOKEN=t\nFAL_KEY=f", catalog);
    expect(plan.providers).toEqual(["newthing"]);
    expect(plan.unknown).toEqual(["FAL_KEY=…"]);
  });

  it("parses a realistic messy paste end to end", () => {
    const text = [
      "# keys, do not commit",
      "",
      "export FAL_KEY='fal-abc-123'",
      'OPENAI_API_KEY="sk-openai-xyz"   # personal',
      "GOOGLE_API_KEY=",
      "SOME_OTHER_TOOL=nope",
      "not a line at all",
    ].join("\r\n");

    const plan = planEnvImport(text);
    expect(plan.assignments).toEqual([
      { provider: "fal", secretHalf: false, value: "fal-abc-123" },
      { provider: "openai", secretHalf: false, value: "sk-openai-xyz" },
    ]);
    expect(plan.unknown).toEqual([
      "not a line at all",
      "GOOGLE_API_KEY= (no value)",
      "SOME_OTHER_TOOL=…",
    ]);
  });
});

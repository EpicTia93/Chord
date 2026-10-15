import { expect, test } from "bun:test";
import type { ChordMacroEntry } from "@chord/dev.improve.chord.lib.typeshare";
import { getEscapeSuggestions } from "#chord-suggestions";
import { normalizeChordTokens } from "#chord-tokens";

const macros: ChordMacroEntry[] = [
  { target: { type: "named", value: "window-size" }, sequence: "1720x864", persistent: true },
  { target: { type: "named", value: "1temporary" }, sequence: "\\1", persistent: false },
  { target: { type: "buffer", value: 9 }, sequence: "", persistent: false },
  { target: { type: "buffer", value: 1 }, sequence: "800x600", persistent: false },
];

function suggest(input: string, entries = macros) {
  return getEscapeSuggestions(normalizeChordTokens([...input]), entries);
}

test("backslash suggests defined macros first, followed by special keys", () => {
  const suggestions = suggest("\\")!.suggestions;
  expect(suggestions.map((item) => item.reference)).toEqual([
    "\\1",
    "\\9",
    "\\=1temporary=",
    "\\=window-size=",
    "\\up",
    "\\left",
    "\\down",
    "\\right",
  ]);
  expect(suggestions.slice(0, 4).map((item) => item.kind)).toEqual([
    "macro",
    "macro",
    "macro",
    "macro",
  ]);
  expect(suggestions[0]!.description).toBe("800x600");
  expect(suggestions[1]!.description).toBe("(empty sequence)");
  expect(suggest("\\", [])!.suggestions.map((item) => item.kind)).toEqual([
    "key",
    "key",
    "key",
    "key",
  ]);
});

test("suggestions narrow as an escape is typed anywhere in the input", () => {
  expect(suggest("/ws\\=win")!.suggestions.map((item) => item.reference)).toEqual([
    "\\=window-size=",
  ]);
  expect(suggest("12/ws\\=1")!.suggestions.map((item) => item.reference)).toEqual([
    "\\=1temporary=",
  ]);
  expect(suggest("0=alias=\\")!.suggestions[0]!.reference).toBe("\\1");
  expect(suggest("a\\u")!.suggestions.map((item) => item.reference)).toEqual(["\\up"]);
  expect(suggest("\\=missing")!.suggestions).toEqual([]);
  expect(suggest("\\=")!.suggestions.map((item) => item.reference)).toEqual([
    "\\=1temporary=",
    "\\=window-size=",
  ]);
  // Real key-code input follows the same path as the displayed chord tokens.
  expect(
    getEscapeSuggestions(normalizeChordTokens(["Slash", "KeyW", "KeyS", "Backslash"]), macros)!
      .suggestions[0]!.reference,
  ).toBe("\\1");
});

test("completed escapes stop suggesting and a later backslash starts a fresh list", () => {
  for (const input of [
    "",
    "/ws",
    "/ws\\1",
    "/ws\\1x",
    "\\=window-size=",
    "\\=window-size=x",
    "\\up",
    "a\\leftb",
  ]) {
    expect(suggest(input)).toBeNull();
  }
  expect(getEscapeSuggestions(normalizeChordTokens(["ArrowUp"]), macros)).toBeNull();
  expect(suggest("\\=window-size=a\\=win")!.suggestions.map((item) => item.reference)).toEqual([
    "\\=window-size=",
  ]);
  expect(suggest("\\1\\")!.suggestions[0]!.reference).toBe("\\1");
});

test("suggestions reflect current definitions without retaining deleted macros", () => {
  expect(suggest("\\=win", macros.slice(1))!.suggestions).toEqual([]);
  expect(
    suggest("\\=win", [{ ...macros[0]!, sequence: "1024x768" }])!.suggestions[0]!.description,
  ).toBe("1024x768");
});

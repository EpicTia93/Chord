import type { ChordMacroEntry } from "@chord/dev.improve.chord.lib.typeshare";

const SPECIAL_KEY_SUGGESTIONS = [
  { reference: "\\up", description: "Up arrow", kind: "key" },
  { reference: "\\left", description: "Left arrow", kind: "key" },
  { reference: "\\down", description: "Down arrow", kind: "key" },
  { reference: "\\right", description: "Right arrow", kind: "key" },
] as const;

export interface EscapeSuggestion {
  reference: string;
  description: string;
  kind: "macro" | "key";
}

/** Tokens have already collapsed complete arrow escapes into physical-key glyphs. */
export function getEscapeSuggestions(tokens: string[], macros: ChordMacroEntry[]) {
  const start = tokens.lastIndexOf("\\");
  if (start === -1) return null;
  const suffix = tokens.slice(start);
  // Completed references no longer need suggestions, even when more keys follow.
  if (/^[1-9]$/.test(suffix[1] ?? "")) return null;
  if (suffix[1] === "=" && suffix.slice(2).includes("=")) return null;
  const prefix = suffix.join("").toLowerCase();
  const macroSuggestions = [...macros]
    .sort((left, right) => {
      if (left.target.type !== right.target.type) return left.target.type === "buffer" ? -1 : 1;
      return String(left.target.value).localeCompare(String(right.target.value), undefined, {
        numeric: true,
      });
    })
    .map((macro): EscapeSuggestion => ({
      reference:
        macro.target.type === "buffer" ? `\\${macro.target.value}` : `\\=${macro.target.value}=`,
      description: macro.sequence || "(empty sequence)",
      kind: "macro",
    }))
    .filter((suggestion) => suggestion.reference.startsWith(prefix));
  return {
    prefix,
    suggestions: [
      ...macroSuggestions,
      ...SPECIAL_KEY_SUGGESTIONS.filter((suggestion) => suggestion.reference.startsWith(prefix)),
    ],
  };
}

import type { ChordMacroEntry } from "@chord/dev.improve.chord.lib.typeshare";

const SPECIAL_KEY_SUGGESTIONS = [
  { reference: "\\up", description: "Up arrow", kind: "key" },
  { reference: "\\left", description: "Left arrow", kind: "key" },
  { reference: "\\down", description: "Down arrow", kind: "key" },
  { reference: "\\right", description: "Right arrow", kind: "key" },
  { reference: "\\n", description: "Enter", kind: "key" },
] as const;

export interface EscapeSuggestion {
  reference: string;
  description: string;
  kind: "macro" | "key";
}

const BUFFER_REFERENCE = /^\\[1-9]$/;

/**
 * Typed escapes never expand on their own: Tab replaces the escape with the
 * `active` suggestion. Macro references expand as soon as they are complete (a
 * buffer's digit or a named macro's closing `=`), so Tab skips numbered buffers.
 */
export function getEscapeSuggestions(tokens: string[], macros: ChordMacroEntry[]) {
  const start = tokens.lastIndexOf("\\");
  if (start === -1) return null;
  const suffix = tokens.slice(start);
  // A numbered buffer reference is complete, even when more keys follow.
  if (/^[1-9]$/.test(suffix[1] ?? "")) return null;
  const prefix = suffix.join("").toLowerCase();
  const macroSuggestions = [...macros]
    .sort((left, right) => {
      if (left.target.type !== right.target.type) return left.target.type === "buffer" ? -1 : 1;
      // Code-unit order, as natively, so the active suggestion matches Tab.
      const [a, b] = [String(left.target.value), String(right.target.value)];
      return a < b ? -1 : a > b ? 1 : 0;
    })
    .map((macro): EscapeSuggestion => ({
      reference:
        macro.target.type === "buffer" ? `\\${macro.target.value}` : `\\=${macro.target.value}=`,
      description: macro.sequence || "(empty sequence)",
      kind: "macro",
    }))
    .filter((suggestion) => suggestion.reference.startsWith(prefix));
  const suggestions = [
    ...macroSuggestions,
    ...SPECIAL_KEY_SUGGESTIONS.filter((suggestion) => suggestion.reference.startsWith(prefix)),
  ];
  return {
    prefix,
    suggestions,
    active: suggestions.find((suggestion) => !BUFFER_REFERENCE.test(suggestion.reference))
      ?.reference,
  };
}

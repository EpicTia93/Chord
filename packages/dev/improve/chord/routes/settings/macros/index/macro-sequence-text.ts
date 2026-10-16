/** Sequences store Enter as `\n` notation; the editor shows it as a line break. */
const ENTER = "\\n";

/**
 * Every backslash in a valid sequence starts an escape, and macro names cannot
 * contain one, so each `\n` in a stored sequence is an Enter key press.
 */
export function sequenceToText(sequence: string) {
  return sequence.replaceAll(ENTER, "\n");
}

export function textToSequence(text: string) {
  return text.replaceAll("\n", ENTER);
}

/** The parts of a stored sequence between Enter key presses. */
export function sequenceLines(sequence: string) {
  return sequence.split(ENTER);
}

import { expect, test } from 'bun:test';
import { getRepeatPrefixTokens, normalizeChordTokens } from '#chord-tokens';

test('typed arrow escapes use the same completion tokens as physical keys', () => {
	for (const [word, arrow] of [['UP', 'ArrowUp'], ['LEFT', 'ArrowLeft'], ['DOWN', 'ArrowDown'], ['RIGHT', 'ArrowRight']]) {
		const typed = ['KeyA', 'Backslash', ...[...word!].map(letter => `Key${letter}`), 'KeyB'];
		expect(normalizeChordTokens(typed)).toEqual(normalizeChordTokens(['KeyA', arrow!, 'KeyB']));
	}
	expect(normalizeChordTokens(['KeyA', 'Backslash', 'KeyU'])).toEqual(['A', '\\', 'U']);
});

test('completion prefixes preserve meta commands and numeric chord arguments', () => {
	expect(getRepeatPrefixTokens([])).toEqual([]);
	expect(getRepeatPrefixTokens(['0', 'H', '1', '2'])).toEqual([]);
	expect(getRepeatPrefixTokens(['1', '2', 'A', 'B', '3', '4'])).toEqual(['1', '2']);
	expect(getRepeatPrefixTokens(['1', '0'])).toEqual(['1', '0']);
	expect(getRepeatPrefixTokens(['A', 'B', '1', '2'])).toEqual([]);
});

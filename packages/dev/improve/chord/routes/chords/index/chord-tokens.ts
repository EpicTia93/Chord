import getPrettyKey from 'pretty-key';

export function normalizeToken(token: string) {
	const pretty = getPrettyKey(token);
	const normalized = pretty === '_' ? '-' : pretty;
	return normalized.length === 1 ? normalized.toUpperCase() : normalized;
}

const SPECIAL_KEY_SEQUENCES = [
	{ tokens: ['\\', 'U', 'P'], key: 'ArrowUp' },
	{ tokens: ['\\', 'L', 'E', 'F', 'T'], key: 'ArrowLeft' },
	{ tokens: ['\\', 'D', 'O', 'W', 'N'], key: 'ArrowDown' },
	{ tokens: ['\\', 'R', 'I', 'G', 'H', 'T'], key: 'ArrowRight' },
];

export function normalizeChordTokens(keys: string[]) {
	const tokens = keys.map(normalizeToken);
	const result: string[] = [];
	for (let index = 0; index < tokens.length;) {
		const special = SPECIAL_KEY_SEQUENCES.find(sequence =>
			sequence.tokens.every((token, offset) => tokens[index + offset] === token));
		if (special) {
			result.push(normalizeToken(special.key));
			index += special.tokens.length;
		} else {
			result.push(tokens[index]!);
			index++;
		}
	}
	return result;
}

export function getRepeatPrefixTokens(tokens: string[]) {
	if (tokens[0] === '0') return [];
	const commandStart = tokens.findIndex(token => !/^\d$/.test(token));
	return tokens.slice(0, commandStart === -1 ? tokens.length : commandStart);
}

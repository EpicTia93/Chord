import type { Chord, ChordHint, ChordReference } from '@chord/dev.improve.chord.lib.typeshare';
import { cn } from '@chord/com.npmjs.utils-cn';
import { Kbd } from '@chord/dev.improve.chord.components.ui.kbd';
import { StateQueries, useChordExecutionState, useChordInputState, useChordPackageManagerState, useChordPanelState, useFrontmostState, useKeyboardState } from '@chord/dev.improve.chord.lib.state';
import { emit, listen } from '@tauri-apps/api/event';
import { getRepeatPrefixTokens, normalizeChordTokens, normalizeToken } from '#chord-tokens';
import { Fragment, useEffect, useLayoutEffect, useRef, useState } from 'react';

const LETTER_TOKENS = Array.from({ length: 26 }, (_, index) =>
	String.fromCharCode('A'.charCodeAt(0) + index));
const MAX_KEY_SIZE = 32;
const NATIVE_SURFACE_RADIUS = 32;
const SHOW_DEVELOPMENT_LABEL = import.meta.env.DEV;
const SINGLE_LETTER_TOKEN_REGEX = /^[A-Z]$/;

function clamp(value: number, min: number, max: number) {
	return Math.min(Math.max(value, min), max);
}

function sortTokens(tokens: Iterable<string>) {
	const tokenSet = new Set(tokens);
	const letterTokens = LETTER_TOKENS.filter(token => tokenSet.has(token));
	const otherTokens = [...tokenSet]
		.filter(token => !SINGLE_LETTER_TOKEN_REGEX.test(token))
		.sort((left, right) => left.localeCompare(right));

	return [...letterTokens, ...otherTokens];
}

function ChordKeyRow({
	token,
	rangeEnd,
	description = '',
	isSelected = false,
	isDimmed = false,
	keySize,
	descriptionFontSize,
}: {
	token: string
	rangeEnd?: string
	description?: string
	isSelected?: boolean
	isDimmed?: boolean
	keySize: number
	descriptionFontSize: number
}) {
	return (
		<div
			className={cn(
				'flex shrink-0 items-center gap-3',
				isDimmed ? 'opacity-35' : 'opacity-100',
				'text-foreground/95',
			)}
		>
			<div className="flex shrink-0 items-center gap-2">
				{(rangeEnd ? [token, rangeEnd] : [token]).map((keyToken, index) => (
					<Fragment key={index}>
						{index > 0 && <span aria-hidden="true">-</span>}
						<Kbd
							style={{
								height: `${keySize}px`,
								minWidth: `${keySize}px`,
								fontSize: `${Math.max(12, Math.round(keySize * 0.48))}px`,
							}}
							className={cn(
								'rounded-md border px-0 font-mono shadow-[inset_0_1px_0_rgba(255,255,255,0.35),0_1px_2px_rgba(0,0,0,0.18)]',
								isSelected
									? 'border-emerald-400/90 bg-emerald-100 text-emerald-950 shadow-[inset_0_1px_0_rgba(255,255,255,0.5),0_0_0_1px_rgba(52,211,153,0.35),0_4px_10px_rgba(16,185,129,0.25)]'
									: 'border-border/80 bg-background/95 text-foreground',
							)}
						>
							{keyToken}
						</Kbd>
					</Fragment>
				))}
			</div>
			<div style={{ fontSize: `${descriptionFontSize}px` }}>{description}</div>
		</div>
	);
}

export function ChordsPage() {
	return (
		<StateQueries queries={[useChordInputState(), useChordPanelState(), useKeyboardState(), useFrontmostState(), useChordPackageManagerState(), useChordExecutionState()]} quiet>
			{(chordInputData, chordPanelData, keyboardData, frontmostData, chordPackageManagerData, chordExecutionData) => (
				<ChordsPageContent chordInputData={chordInputData} chordPanelData={chordPanelData} keyboardData={keyboardData} frontmostData={frontmostData} chordPackageManagerData={chordPackageManagerData} chordExecutionData={chordExecutionData} />
			)}
		</StateQueries>
	);
}

function ChordsPageContent({
	chordInputData,
	chordPanelData,
	keyboardData,
	frontmostData,
	chordPackageManagerData,
	chordExecutionData,
}: {
	chordInputData: NonNullable<ReturnType<typeof useChordInputState>['data']>
	chordPanelData: NonNullable<ReturnType<typeof useChordPanelState>['data']>
	keyboardData: NonNullable<ReturnType<typeof useKeyboardState>['data']>
	frontmostData: NonNullable<ReturnType<typeof useFrontmostState>['data']>
	chordPackageManagerData: NonNullable<ReturnType<typeof useChordPackageManagerState>['data']>
	chordExecutionData: NonNullable<ReturnType<typeof useChordExecutionState>['data']>
}) {
	const chordInputState = chordInputData;
	const chordPanelState = chordPanelData;
	const keyboardState = keyboardData;
	const { frontmostAppBundleId } = frontmostData;
	const { packages } = chordPackageManagerData;
	const { loops, metaCommands } = chordExecutionData;

	const [viewportHeight, setViewportHeight] = useState(() => window.innerHeight);
	const [surfaceVersion, setSurfaceVersion] = useState(0);
	const surfaceRef = useRef<HTMLDivElement>(null);

	const emitSurfaceRect = () => {
		const surface = surfaceRef.current;
		if (!surface) {
			return;
		}

		const rect = surface.getBoundingClientRect();
		void emit('chorder-surface-rect', {
			x: rect.left,
			y: window.innerHeight - rect.bottom,
			width: rect.width,
			height: rect.height,
			radius: NATIVE_SURFACE_RADIUS,
		});
	};

	useEffect(() => {
		const handleResize = () => {
			setViewportHeight(window.innerHeight);
		};

		window.addEventListener('resize', handleResize);
		return () => {
			window.removeEventListener('resize', handleResize);
		};
	}, []);

	useEffect(() => {
		const unlistenPromise = listen('chorder-will-show', () => {
			setSurfaceVersion(version => version + 1);
		});

		return () => {
			void unlistenPromise.then(unlisten => unlisten?.());
		};
	}, []);

	useEffect(() => {
		void emit('chorder-window-ready');
	}, []);

	useLayoutEffect(() => {
		if (surfaceVersion === 0) {
			return;
		}

		emitSurfaceRect();
		void emit('chorder-surface-ready');
	}, [surfaceVersion]);

	useEffect(() => {
		const surface = surfaceRef.current;
		if (!surface) {
			return;
		}

		const observer = new ResizeObserver(() => {
			emitSurfaceRect();
		});
		observer.observe(surface);

		return () => {
			observer.disconnect();
		};
	}, [surfaceVersion]);

	const activeAppChords: Chord[] = [];
	const hintsByRawPattern: Record<string, ChordHint> = {};
	const globalChords: ChordReference[] = [];

	for (const chordPackage of packages) {
		globalChords.push(...chordPackage.globalChords);

		for (const [relpath, file] of Object.entries(chordPackage.compiledChordsFiles)) {
			const bundleId = relpath.split('/').slice(1, -1).join('.');
			for (const hint of file.chordHints) {
				// bad check for global
				if (hint.rawPattern[0]?.toUpperCase() === hint.rawPattern[0]) {
					hintsByRawPattern[hint.rawPattern] = hint;
				}
			}

			if (bundleId === frontmostAppBundleId) {
				for (const hint of file.chordHints) {
					hintsByRawPattern[hint.rawPattern] = hint;
				}

				for (const chord of file.chords) {
					activeAppChords.push(chord);
				}
			}
		}
	}

	const activeChords: Chord[] = [...activeAppChords, ...globalChords.map(c => c.chord)];
	const baseHintSequences = Object.values(hintsByRawPattern).flatMap(hint =>
		'keys' in hint.pattern
			? [{ tokens: hint.pattern.keys.map(normalizeToken), description: hint.description }]
			: 'range' in hint.pattern
				? [{ tokens: hint.pattern.range.prefix.map(normalizeToken), description: '' }]
				: [],
	);

	const baseRangeHints = Object.values(hintsByRawPattern).flatMap(hint =>
		'range' in hint.pattern
			? [
					{
						prefix: hint.pattern.range.prefix.map(normalizeToken),
						keys: hint.pattern.range.keys.map(normalizeToken),
						description: hint.description,
					},
				]
			: [],
	);

	const normalizedBufferTokens = normalizeChordTokens(chordInputState.input);
	const normalizedActiveChordTokens = normalizeChordTokens(chordInputState.selectedInputEvent?.input ?? []);

	const shouldHighlightActiveChord
		= keyboardState.isShiftPressed
			&& normalizedBufferTokens.length === 0
			&& normalizedActiveChordTokens.length > 0;
	const selectedTokens = shouldHighlightActiveChord
		? normalizedActiveChordTokens
		: normalizedBufferTokens;
	const currentPrefixLength = selectedTokens.length;
	const repeatPrefixTokens = getRepeatPrefixTokens(selectedTokens);
	const builtinHints = [
		...metaCommands,
		...loops.map(loop => ({ sequence: `0h${loop.id}`, name: `Halt loop ${loop.id}: ${loop.input}` })),
	].map(hint => ({ tokens: [...hint.sequence].map(normalizeToken), description: hint.name }));
	const hintSequences = [
		...baseHintSequences.map(hint => ({ ...hint, tokens: [...repeatPrefixTokens, ...hint.tokens] })),
		...(repeatPrefixTokens.length === 0 ? builtinHints : []),
		...Array.from({ length: repeatPrefixTokens.length ? 10 : 9 }, (_, index) => {
			const digit = String(index + (repeatPrefixTokens.length ? 0 : 1));
			return { tokens: [...repeatPrefixTokens, digit], description: repeatPrefixTokens.length === 0 && digit === '1' ? 'Loop chord' : 'Repeat count' };
		}),
	];
	const rangeHints = baseRangeHints.map(hint => ({ ...hint, prefix: [...repeatPrefixTokens, ...hint.prefix] }));

	const maxVisibleRows = 20;
	const availableHeight = Math.max(viewportHeight - 96, 240);
	const idealKeySize = availableHeight / (maxVisibleRows + Math.max(maxVisibleRows - 1, 0) * 0.18);
	const keySize = clamp(Math.floor(idealKeySize), 22, MAX_KEY_SIZE);
	const rowGap = clamp(
		Math.floor((availableHeight - keySize * maxVisibleRows) / Math.max(maxVisibleRows - 1, 1)),
		4,
		10,
	);
	const descriptionFontSize = clamp(Math.round(keySize * 0.42), 11, 16);

	const keyColumns = Array.from(
		{
			length: shouldHighlightActiveChord
				? Math.max(1, currentPrefixLength)
				: Math.max(1, currentPrefixLength + 1),
		},
		(_, columnIndex) => {
			const prefixTokens = selectedTokens.slice(0, columnIndex);
			const getChordKeys = (chord: Chord) => 'keys' in chord.trigger ? [...repeatPrefixTokens, ...normalizeChordTokens(chord.trigger.keys)] : [];

			const matchingChords = activeChords.filter(chord =>
				prefixTokens.every((token, tokenIndex) => getChordKeys(chord)[tokenIndex] === token),
			);
			const activeTokens = new Set(
				matchingChords
					.map(chord => getChordKeys(chord)[columnIndex])
					.filter((token): token is string => Boolean(token)),
			);
			// Explicit hints are options even when their handler uses a regex trigger.
			const matchingHints = hintSequences.filter(hint =>
				prefixTokens.every((token, tokenIndex) => hint.tokens[tokenIndex] === token),
			);
			for (const hint of matchingHints) {
				const token = hint.tokens[columnIndex];
				if (token) {
					activeTokens.add(token);
				}
			}

			const matchingRanges = rangeHints.filter(
				hint =>
					hint.prefix.length === columnIndex &&
					prefixTokens.every((token, index) => hint.prefix[index] === token),
			);
			for (const range of matchingRanges) {
				for (const token of range.keys) activeTokens.delete(token);
			}

			const rows = sortTokens(activeTokens).map(token => {
				const sequenceKey = [...prefixTokens, token].join('').toLowerCase();
				const exactHint = matchingHints.findLast(
					hint => hint.tokens[columnIndex] === token && hint.tokens.length === columnIndex + 1,
				);
				const exactChord = matchingChords.find(
					chord => getChordKeys(chord)[columnIndex] === token && getChordKeys(chord).length === columnIndex + 1,
				);

				return {
					token,
					rangeEnd: undefined as string | undefined,
					keys: [token],
					description: exactHint?.description ?? hintsByRawPattern[sequenceKey]?.description ?? exactChord?.name ?? '',
				};
			});

			for (const range of matchingRanges) {
				rows.push({
					token: range.keys[0]!,
					rangeEnd: range.keys.at(-1)!,
					keys: range.keys,
					description: range.description,
				});
			}
			const tokenOrder = sortTokens(rows.map(row => row.token));
			rows.sort((left, right) => tokenOrder.indexOf(left.token) - tokenOrder.indexOf(right.token));

			return {
				id: `column-${columnIndex}`,
				rows,
				selectedToken: selectedTokens[columnIndex],
				hasSelection: Boolean(selectedTokens[columnIndex]),
			};
		},
	);

	useLayoutEffect(() => {
		emitSurfaceRect();
	}, [currentPrefixLength, keyColumns.length, keySize, rowGap, descriptionFontSize, chordPanelState.isVisible]);

	return (
		<div className="relative size-full bg-transparent">
			<div className="absolute left-0 top-1/2 -translate-y-1/2">
				<div
					key={surfaceVersion}
					ref={surfaceRef}
					className={cn(
						'relative isolate overflow-hidden rounded-r-[2rem] rounded-l-none border border-l-0 px-5 py-5 pl-7',
						'border-white/30 bg-white/22 shadow-[18px_20px_60px_rgba(15,23,42,0.18),inset_0_1px_0_rgba(255,255,255,0.42)]',
						'dark:border-white/10 dark:bg-zinc-950/24 dark:shadow-[18px_20px_60px_rgba(0,0,0,0.34),inset_0_1px_0_rgba(255,255,255,0.1)]',
					)}
					style={{
						// Move the native vibrancy surface offscreen too when toggled off.
						transform: chordPanelState.isVisible ? undefined : 'translateX(calc(-100% - 40px))',
						opacity: chordPanelState.isVisible ? 1 : 0,
					}}
				>
						{repeatPrefixTokens.length > 0 && (
							<div className="mb-3 text-xs text-foreground/70">
								{Number(repeatPrefixTokens.join('')) === 1 ? 'Loop chord' : `Repeat ${repeatPrefixTokens.join('')} times`}
							</div>
						)}
						{loops.length > 0 && (
							<div className="mb-3 flex max-h-24 flex-col gap-1 overflow-y-auto text-xs text-foreground/80">
								{loops.map(loop => (
									<div key={loop.id}>Loop {loop.id}: <code>{loop.input}</code> · Halt: <code>0h{loop.id}</code></div>
								))}
							</div>
						)}
					<div className="relative flex items-start">
						<div className="flex items-start gap-6">
							{keyColumns.map(column => (
								<div
									key={column.id}
									className="flex flex-col items-start overflow-y-auto px-1 py-1"
									style={{ gap: `${rowGap}px`, maxHeight: `${availableHeight}px` }}
								>
									{SHOW_DEVELOPMENT_LABEL && column.id === 'column-0'
										? (
												<div className="-mb-1 text-[10px] font-semibold tracking-[0.28em] text-foreground/55">
													DEVELOPMENT
												</div>
											)
										: null}
									{column.rows.map(row => (
										<ChordKeyRow
											key={`${column.id}-${row.token}-${row.rangeEnd ?? ''}`}
											token={row.token}
											rangeEnd={row.rangeEnd}
											description={row.description}
											isSelected={row.keys.includes(column.selectedToken)}
											isDimmed={column.hasSelection && !row.keys.includes(column.selectedToken)}
											keySize={keySize}
											descriptionFontSize={descriptionFontSize}
										/>
									))}
								</div>
							))}
						</div>
					</div>
				</div>
			</div>
		</div>
	);
}

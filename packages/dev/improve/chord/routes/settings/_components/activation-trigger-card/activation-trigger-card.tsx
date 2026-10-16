import { useMutation, useQuery } from '@chord/com.npmjs.tanstack__react-query';
import { taurpc } from '@chord/dev.improve.chord.api.taurpc';
import { Button } from '@chord/dev.improve.chord.components.ui.button';
import {
	Card,
	CardContent,
	CardDescription,
	CardHeader,
	CardTitle,
} from '@chord/dev.improve.chord.components.ui.card';
import { ShortcutKeys } from '@chord/dev.improve.chord.routes.settings._components.shortcut-keys';
import { TriangleAlert } from 'lucide-react';

export function ActivationTriggerCard() {
	return (
		<Card size="sm">
			<CardHeader>
				<CardTitle>Activation Trigger</CardTitle>
				<CardDescription>
					Use this keyboard trigger to open the chord overlay anywhere in macOS.
				</CardDescription>
			</CardHeader>
			<CardContent className="space-y-2 pt-0">
				<div className="flex items-center justify-between gap-3 rounded-lg border bg-background/80 px-3 py-3">
					<div className="min-w-0">
						<p className="font-medium">Open Chord Overlay</p>
						<p className="text-xs text-muted-foreground">
							This shortcut is fixed for now and shown here for reference.
						</p>
					</div>
					<div className="shrink-0 rounded-lg border bg-muted/50 px-2 py-1">
						<ShortcutKeys shortcut="capsLock+space" />
					</div>
				</div>
				<VoiceOverWarning />
			</CardContent>
		</Card>
	);
}

/** VoiceOver can't use the Caps Lock key as its modifier while Chord owns the key. */
function VoiceOverWarning() {
	const { data: usesCapsLock } = useQuery({
		queryKey: ['voice-over-uses-caps-lock'],
		queryFn: () => taurpc.voiceOverUsesCapsLock(),
		// VoiceOver doesn't announce setting changes; check again when the user returns.
		refetchOnWindowFocus: 'always',
	});
	const openUtilityMutation = useMutation({
		mutationFn: () => taurpc.openVoiceOverUtility(),
	});

	if (usesCapsLock !== true) {
		return null;
	}

	return (
		<div className="flex items-center justify-between gap-3 rounded-lg border border-amber-500/40 bg-amber-500/10 px-3 py-2">
			<div className="flex min-w-0 gap-2">
				<TriangleAlert className="mt-0.5 size-4 shrink-0 text-amber-600" />
				<div className="min-w-0">
					<p className="font-medium">VoiceOver can't use Caps Lock</p>
					<p className="text-xs text-muted-foreground">
						Chord takes over the Caps Lock key, so it no longer works as the VoiceOver modifier.
						In VoiceOver Utility, set the VoiceOver modifier to Control-Option.
					</p>
				</div>
			</div>
			<Button
				type="button"
				variant="outline"
				size="sm"
				className="shrink-0"
				onClick={() => {
					openUtilityMutation.mutate();
				}}
				disabled={openUtilityMutation.isPending}
			>
				{openUtilityMutation.isPending ? 'Opening…' : 'Open VoiceOver Utility'}
			</Button>
		</div>
	);
}

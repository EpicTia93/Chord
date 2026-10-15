import type { ChordMacroEntry, ChordMacroName } from "@chord/dev.improve.chord.lib.typeshare";
import { useMutation } from "@chord/com.npmjs.tanstack__react-query";
import { taurpc } from "@chord/dev.improve.chord.api.taurpc";
import { Badge } from "@chord/dev.improve.chord.components.ui.badge";
import { Button } from "@chord/dev.improve.chord.components.ui.button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@chord/dev.improve.chord.components.ui.card";
import { Input } from "@chord/dev.improve.chord.components.ui.input";
import { StateQueries, useChordMacrosState } from "@chord/dev.improve.chord.lib.state";
import { Pencil, Trash2 } from "lucide-react";
import { useState } from "react";

export function SettingsMacrosPage() {
  return (
    <StateQueries queries={[useChordMacrosState()]}>
      {(state) => <MacrosContent entries={state.entries} />}
    </StateQueries>
  );
}

function MacrosContent({ entries }: { entries: ChordMacroEntry[] }) {
  const named = entries.filter((entry) => entry.target.type === "named");
  return (
    <div className="space-y-4">
      <Card size="sm">
        <CardHeader>
          <CardTitle>Named Macros</CardTitle>
          <CardDescription>
            Expand a macro with <code>{"\\=name="}</code>. Names starting with a digit clear when
            Chord restarts; other names are saved.
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-4">
          <NamedMacroEditor />
          <div className="space-y-2 border-t pt-4">
            {named.length === 0 ? (
              <p className="text-sm text-muted-foreground">No named macros yet.</p>
            ) : (
              named.map((entry) => <NamedMacroRow key={JSON.stringify(entry)} entry={entry} />)
            )}
          </div>
        </CardContent>
      </Card>
      <Card size="sm">
        <CardHeader>
          <CardTitle>Temporary Buffers</CardTitle>
          <CardDescription>
            Use <code>{"\\1"}</code> through <code>{"\\9"}</code> to expand these buffers. They
            clear when Chord restarts.
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-2">
          {Array.from({ length: 9 }, (_, index) => {
            const digit = index + 1;
            const entry = entries.find(
              (candidate) => candidate.target.type === "buffer" && candidate.target.value === digit,
            );
            return (
              <BufferRow key={`${digit}:${entry?.sequence ?? ""}`} digit={digit} entry={entry} />
            );
          })}
        </CardContent>
      </Card>
    </div>
  );
}

function NamedMacroEditor({ entry, onDone }: { entry?: ChordMacroEntry; onDone?: () => void }) {
  const [name, setName] = useState(entry?.target.type === "named" ? entry.target.value : "");
  const [sequence, setSequence] = useState(entry?.sequence ?? "");
  const save = useMutation({
    mutationFn: () =>
      taurpc.setChordMacro({ type: "named", value: name }, sequence, entry?.target ?? null),
    onSuccess: () => {
      if (onDone) onDone();
      else {
        setName("");
        setSequence("");
      }
    },
  });
  return (
    <form
      className="space-y-2"
      onSubmit={(event) => {
        event.preventDefault();
        save.mutate();
      }}
    >
      <div className="grid gap-2 sm:grid-cols-[minmax(0,1fr)_minmax(0,1.5fr)_auto] sm:items-end">
        <label className="min-w-0 space-y-1 text-xs text-muted-foreground">
          <span>Name</span>
          <Input
            value={name}
            onChange={(event) => setName(event.target.value)}
            required
            placeholder="window-size"
            spellCheck={false}
            autoCapitalize="none"
            autoCorrect="off"
            aria-label={entry ? `Name for ${reference(entry.target)}` : "New macro name"}
            disabled={save.isPending}
          />
        </label>
        <label className="min-w-0 space-y-1 text-xs text-muted-foreground">
          <span>Sequence</span>
          <Input
            value={sequence}
            onChange={(event) => setSequence(event.target.value)}
            placeholder="1720x864"
            spellCheck={false}
            autoCapitalize="none"
            autoCorrect="off"
            aria-label={entry ? `Sequence for ${reference(entry.target)}` : "New macro sequence"}
            className="font-mono"
            disabled={save.isPending}
          />
        </label>
        <div className="flex items-center gap-2">
          {onDone && (
            <Button type="button" variant="outline" onClick={onDone} disabled={save.isPending}>
              Cancel
            </Button>
          )}
          <Button type="submit" disabled={!name || save.isPending}>
            {save.isPending ? "Saving…" : entry ? "Save" : "Add macro"}
          </Button>
        </div>
      </div>
      {name && (
        <p className="text-xs text-muted-foreground">
          <code>{`\\=${name}=`}</code> ·{" "}
          {/^[0-9]/.test(name)
            ? "Temporary — clears on restart"
            : "Persistent — saved across restarts"}
        </p>
      )}
      {save.isError && <MacroError error={save.error} />}
    </form>
  );
}

function NamedMacroRow({ entry }: { entry: ChordMacroEntry }) {
  const [editing, setEditing] = useState(false);
  const remove = useMutation({ mutationFn: () => taurpc.removeChordMacro(entry.target) });
  return (
    <div className="rounded-lg border bg-background/80 p-3">
      {editing ? (
        <NamedMacroEditor entry={entry} onDone={() => setEditing(false)} />
      ) : (
        <div className="flex items-center gap-3">
          <div className="min-w-0 flex-1 space-y-1">
            <div className="flex flex-wrap items-center gap-2">
              <code className="break-all text-sm font-medium">{reference(entry.target)}</code>
              <Badge variant="outline">{entry.persistent ? "Persistent" : "Temporary"}</Badge>
            </div>
            <p className="break-all font-mono text-xs text-muted-foreground">
              {entry.sequence || "(empty sequence)"}
            </p>
          </div>
          <Button
            type="button"
            size="icon-sm"
            variant="ghost"
            aria-label={`Edit ${reference(entry.target)}`}
            onClick={() => setEditing(true)}
            disabled={remove.isPending}
          >
            <Pencil />
          </Button>
          <Button
            type="button"
            size="icon-sm"
            variant="ghost"
            aria-label={`Delete ${reference(entry.target)}`}
            onClick={() => remove.mutate()}
            disabled={remove.isPending}
            className="text-muted-foreground hover:text-destructive"
          >
            <Trash2 />
          </Button>
        </div>
      )}
      {remove.isError && <MacroError error={remove.error} />}
    </div>
  );
}

function BufferRow({ digit, entry }: { digit: number; entry?: ChordMacroEntry }) {
  const [sequence, setSequence] = useState(entry?.sequence ?? "");
  const target: ChordMacroName = { type: "buffer", value: digit };
  const save = useMutation({ mutationFn: () => taurpc.setChordMacro(target, sequence, null) });
  const clear = useMutation({ mutationFn: () => taurpc.removeChordMacro(target) });
  const pending = save.isPending || clear.isPending;
  const dirty = sequence !== (entry?.sequence ?? "");
  return (
    <form
      onSubmit={(event) => {
        event.preventDefault();
        save.mutate();
      }}
      className="space-y-1"
    >
      <div className="flex items-center gap-2">
        <label
          htmlFor={`macro-buffer-${digit}`}
          className="w-7 shrink-0 font-mono text-sm"
        >{`\\${digit}`}</label>
        <Input
          id={`macro-buffer-${digit}`}
          aria-label={`Buffer ${digit} sequence`}
          value={sequence}
          onChange={(event) => setSequence(event.target.value)}
          placeholder="Empty"
          className="min-w-0 flex-1 font-mono"
          spellCheck={false}
          autoCapitalize="none"
          autoCorrect="off"
          disabled={pending}
        />
        {dirty && (
          <Button type="submit" size="sm" disabled={pending}>
            {save.isPending ? "Saving…" : "Save"}
          </Button>
        )}
        <Button
          type="button"
          variant="ghost"
          size="sm"
          aria-label={`Clear buffer ${digit}`}
          disabled={pending || !entry}
          onClick={() => clear.mutate()}
        >
          Clear
        </Button>
      </div>
      {(save.isError || clear.isError) && <MacroError error={save.error ?? clear.error} />}
    </form>
  );
}

function reference(target: ChordMacroName) {
  return target.type === "buffer" ? `\\${target.value}` : `\\=${target.value}=`;
}

function MacroError({ error }: { error: unknown }) {
  const message =
    typeof error === "string"
      ? error
      : error instanceof Error
        ? error.message
        : error && typeof error === "object" && "Message" in error
          ? String(error.Message)
          : "Could not update this macro. Try again.";
  return (
    <p role="alert" className="text-xs text-destructive">
      {message}
    </p>
  );
}

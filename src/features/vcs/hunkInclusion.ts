import { StateEffect, StateField, type Extension } from "@codemirror/state";
import { EditorView, ViewPlugin, type ViewUpdate } from "@codemirror/view";
import type { LineHunk } from "@/bindings";
import type { MenuItem } from "@/ui";
import { commands } from "@/platform/ipc";
import { checkOf, hunkIncluded, toggledHunk } from "./changes";
import { useChangelists, withListDefaults } from "./changelists";
import { useCommit } from "./commitStore";
import { hunkAtLine, hunkKeys } from "./hunks";
import { statusOf, useVcs } from "./store";

interface Blocks {
  hunks: readonly LineHunk[];
  keys: readonly string[];
}

const setBlocks = StateEffect.define<Blocks>();
const blocksField = StateField.define<Blocks | null>({
  create: () => null,
  update: (value, tr) => tr.effects.find((effect) => effect.is(setBlocks))?.value ?? (tr.docChanged ? null : value),
});

/** The effective inclusion of `path` (change list defaults applied). */
function inclusionOf(path: string) {
  const fs = statusOf(path);
  if (!fs) return null;
  const { status, root } = useVcs.getState();
  const { checks, excludedHunks } = useCommit.getState();
  const effective = withListDefaults(status, root ?? "", useChangelists.getState().state, checks);
  return { key: fs.path, check: checkOf(fs, effective), excluded: excludedHunks.get(fs.path) };
}

/** Includes / excludes one changed block of `path`; `all` are the keys of every block. */
function toggleBlock(path: string, all: readonly string[], key: string) {
  const cur = inclusionOf(path);
  if (!cur) return;
  const next = toggledHunk(cur.check, cur.excluded, all, key);
  const { checks, excludedHunks } = useCommit.getState();
  const nextChecks = new Map(checks).set(cur.key, next.check);
  const nextExcluded = new Map(excludedHunks);
  if (next.excluded.size) nextExcluded.set(cur.key, next.excluded);
  else nextExcluded.delete(cur.key);
  useCommit.setState({ checks: nextChecks, excludedHunks: nextExcluded });
}

/** Menu action for the backend hunk under the pointer. Partial commits use the same hunk keys. */
export function hunkInclusionItem(view: EditorView, path: string, line: number): MenuItem | undefined {
  const blocks = view.state.field(blocksField, false);
  const cur = inclusionOf(path);
  if (!blocks || !cur) return;
  const hunk = hunkAtLine(blocks.hunks, line, view.state.doc.lines);
  if (!hunk) return;
  const key = blocks.keys[blocks.hunks.indexOf(hunk)]!;
  const included = hunkIncluded(cur.check, cur.excluded, key);
  const doc = view.state.doc;
  return {
    label: included ? "Exclude Change from Commit" : "Include Change in Commit",
    run: () => {
      if (view.state.doc === doc) toggleBlock(path, blocks.keys, key);
    },
  };
}

/** Keep partial-commit actions based on the backend's hunks for the current text. */
export function hunkInclusion(base: string): Extension {
  const compute = ViewPlugin.fromClass(
    class {
      timer: ReturnType<typeof setTimeout> | null = null;
      live = true;

      constructor(readonly view: EditorView) {
        this.schedule(0);
      }
      update(u: ViewUpdate) {
        if (u.docChanged) this.schedule(250);
      }
      schedule(delay: number) {
        if (this.timer) clearTimeout(this.timer);
        this.timer = setTimeout(() => void this.run(), delay);
      }
      async run() {
        const doc = this.view.state.doc;
        const text = doc.toString();
        let hunks: LineHunk[];
        try {
          hunks = await commands.vcsDiffHunks(base, text);
        } catch {
          return;
        }
        if (!this.live || this.view.state.doc !== doc) return;
        this.view.dispatch({ effects: setBlocks.of({ hunks, keys: hunkKeys(text, hunks) }) });
      }
      destroy() {
        if (this.timer) clearTimeout(this.timer);
        this.live = false;
      }
    },
  );
  return [blocksField, compute];
}

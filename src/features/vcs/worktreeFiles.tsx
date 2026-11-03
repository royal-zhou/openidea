/**
 * Browse and open files of a selected Git worktree, and switch this window to it.
 */
import { useEffect, useState, type ReactNode } from "react";
import type { FileEntry } from "@/bindings";
import { call, commands } from "@/platform/ipc";
import { confirmLeaveProject, openProject, useProject } from "@/platform/project";
import { tx } from "@/platform/i18n";
import { Icon, T, fileIconName } from "@/ui";
import { editorService } from "@/workbench/contributions";
import { errorMessage } from "./ops";
import styles from "./common.module.css";

/** Trailing separators do not make two worktree roots different. */
export function sameWorktreePath(a: string | null | undefined, b: string | null | undefined): boolean {
  if (!a || !b) return false;
  return a.replace(/[\\/]+$/, "") === b.replace(/[\\/]+$/, "");
}

export function isWorktreeDir(entry: Pick<FileEntry, "kind" | "targetKind">): boolean {
  return entry.kind === "dir" || entry.targetKind === "dir";
}

/** Source files of a worktree listing: skip hidden and ignored names, directories first. */
export function visibleWorktreeEntries(entries: readonly FileEntry[]): FileEntry[] {
  return entries
    .filter((e) => !e.hidden && !e.ignored)
    .sort((a, b) => {
      const rank = Number(isWorktreeDir(b)) - Number(isWorktreeDir(a));
      return rank || a.name.localeCompare(b.name);
    });
}

/**
 * Makes `path` this window's project so its tree and editors show that worktree.
 * A worktree already open in another window is focused instead.
 */
export async function switchToWorktree(path: string): Promise<"same" | "focused" | "opened" | "cancelled"> {
  if (sameWorktreePath(useProject.getState().project?.root, path)) return "same";
  const outcome = await call(commands.windowOpenProject(path, false, []));
  if (outcome.kind === "focused") return "focused";
  if (!(await confirmLeaveProject())) return "cancelled";
  await openProject(path);
  return "opened";
}

export function WorktreeFiles({ root }: { root: string }) {
  const [open, setOpen] = useState<Set<string>>(() => new Set());
  const [dirs, setDirs] = useState<Map<string, FileEntry[]>>(() => new Map());
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    call(commands.fsReadDir(root))
      .then((entries) => {
        if (!cancelled) setDirs(new Map([[root, visibleWorktreeEntries(entries)]]));
      })
      .catch((e) => {
        if (!cancelled) setError(errorMessage(e));
      });
    return () => {
      cancelled = true;
    };
  }, [root]);

  const toggleDir = (path: string) => {
    setOpen((cur) => {
      const next = new Set(cur);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
    if (!dirs.has(path)) {
      call(commands.fsReadDir(path))
        .then((entries) => setDirs((cur) => new Map(cur).set(path, visibleWorktreeEntries(entries))))
        .catch((e) => setError(errorMessage(e)));
    }
  };

  const rows = (path: string, depth: number): ReactNode => {
    const entries = dirs.get(path);
    if (!entries) return null;
    return entries.map((entry) => {
      const dir = isWorktreeDir(entry);
      const expanded = open.has(entry.path);
      return (
        <div key={entry.path}>
          <button
            type="button"
            className={styles.worktreeFile}
            style={{ paddingLeft: 8 + depth * 14 }}
            onClick={() => {
              if (dir) toggleDir(entry.path);
              else
                void editorService()
                  .openFile(entry.path, { focus: true })
                  .catch((e) => setError(errorMessage(e)));
            }}
          >
            <Icon name={dir ? (expanded ? "folder-open" : "folder") : fileIconName(entry.name)} size={16} />
            <span className={styles.fileName}>{entry.name}</span>
          </button>
          {dir && expanded && rows(entry.path, depth + 1)}
        </div>
      );
    });
  };

  return (
    <div className={styles.worktreeFiles} aria-label={tx("Worktree files")}>
      <div className={styles.worktreeFilesTitle}>
        <T>Worktree files</T>
      </div>
      {error ? <div className={styles.error}>{error}</div> : rows(root, 0)}
    </div>
  );
}

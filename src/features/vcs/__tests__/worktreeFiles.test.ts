import { describe, expect, it } from "vitest";
import type { FileEntry } from "@/bindings";
import { isWorktreeDir, sameWorktreePath, visibleWorktreeEntries } from "../worktreeFiles";

const entry = (name: string, kind: FileEntry["kind"], extra: Partial<FileEntry> = {}): FileEntry => ({
  name,
  path: `/wt/${name}`,
  kind,
  size: 0,
  mtimeMs: null,
  ignored: false,
  excluded: false,
  hidden: false,
  ...extra,
});

describe("worktree paths", () => {
  it("treats a trailing separator as the same worktree", () => {
    expect(sameWorktreePath("/repo/app", "/repo/app/")).toBe(true);
    expect(sameWorktreePath("/repo/app", "/repo/app-fix")).toBe(false);
    expect(sameWorktreePath(null, "/repo/app")).toBe(false);
  });
});

describe("visibleWorktreeEntries", () => {
  it("hides ignored and hidden names and lists directories first", () => {
    const entries = visibleWorktreeEntries([
      entry("notes.txt", "file"),
      entry("node_modules", "dir", { ignored: true }),
      entry(".git", "dir", { hidden: true }),
      entry("src", "dir"),
      entry("link", "symlink", { targetKind: "dir" }),
    ]);
    expect(entries.map((e) => e.name)).toEqual(["link", "src", "notes.txt"]);
    expect(isWorktreeDir(entries[0]!)).toBe(true);
    expect(isWorktreeDir(entries[2]!)).toBe(false);
  });
});

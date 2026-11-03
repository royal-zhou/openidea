import { beforeEach, describe, expect, it, vi } from "vitest";
import type { FileStatus } from "@/bindings";

const vcsUnstage = vi.fn();
const fsDelete = vi.fn();

vi.mock("@/platform/ipc", async (importOriginal) => {
  const real = await importOriginal<typeof import("@/platform/ipc")>();
  return {
    ...real,
    call: (p: unknown) => p,
    commands: {
      ...real.commands,
      vcsUnstage: (paths: string[]) => vcsUnstage(paths),
      fsDelete: (path: string) => fsDelete(path),
    },
  };
});

const { deleteCandidates, deleteNewFiles, isNewFile } = await import("../changeOps");
const { useVcs } = await import("../store");

const fs = (rel: string, staged: FileStatus["staged"], unstaged: FileStatus["unstaged"]): FileStatus => ({
  path: `/p/${rel}`,
  relPath: rel,
  staged,
  unstaged,
  renamedFrom: null,
});

const status = [
  fs("new.txt", null, "untracked"),
  fs("added.txt", "added", null),
  fs("edited.txt", null, "modified"),
  fs("staged.txt", "modified", null),
];

beforeEach(() => {
  vcsUnstage.mockReset().mockResolvedValue(undefined);
  fsDelete.mockReset().mockResolvedValue(undefined);
  // `available` stays false: the refresh after deleting then has nothing to query.
  useVcs.setState({ status, available: false });
});

describe("deleting new files", () => {
  it("offers only unversioned and indexed-as-new files", () => {
    expect(status.filter(isNewFile).map((f) => f.relPath)).toEqual(["new.txt", "added.txt"]);
    expect(deleteCandidates(["/p/new.txt", "/p/edited.txt", "/p/added.txt"], status).map((f) => f.relPath)).toEqual([
      "new.txt",
      "added.txt",
    ]);
    expect(deleteCandidates(["/p/edited.txt", "/p/staged.txt"], status)).toEqual([]);
    expect(deleteCandidates([], status)).toEqual([]);
  });

  it("unstages added files before deleting them from disk", async () => {
    const count = await deleteNewFiles(["/p/new.txt", "/p/added.txt"]);
    expect(count).toBe(2);
    expect(vcsUnstage).toHaveBeenCalledWith(["/p/added.txt"]);
    expect(fsDelete.mock.calls).toEqual([["/p/new.txt"], ["/p/added.txt"]]);
    // Nothing is deleted when the selection holds no new file.
    fsDelete.mockClear();
    expect(await deleteNewFiles(["/p/edited.txt"])).toBe(0);
    expect(fsDelete).not.toHaveBeenCalled();
  });
});

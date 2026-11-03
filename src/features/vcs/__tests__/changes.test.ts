import { describe, expect, it } from "vitest";
import type { FileStatus } from "@/bindings";
import {
  aggregateCheck,
  buildChangeTree,
  commitRepos,
  ignoreTargets,
  initialCheck,
  hunkIncluded,
  planCommit,
  stagingNodeChecked,
  stagingPlan,
  stagingToggle,
  toggledHunk,
  withStagedChecks,
  type CheckState,
  type ChangeNode,
} from "../changes";
import { rootOf } from "../model";

const fs = (rel: string, staged: FileStatus["staged"], unstaged: FileStatus["unstaged"]): FileStatus => ({
  path: `/r/${rel}`,
  relPath: rel,
  staged,
  unstaged,
  renamedFrom: null,
});

const status = [
  fs("src/main/kotlin/App.kt", null, "modified"),
  fs("src/main/kotlin/Partial.kt", "modified", "modified"),
  fs("src/test/AppTest.kt", "added", null),
  fs("README.md", "modified", null),
  fs("notes.txt", null, "untracked"),
  fs("conf.yml", null, "conflicted"),
  fs("out/", null, "ignored"),
];

const shape = (nodes: ChangeNode[], depth = 0): string[] =>
  nodes.flatMap((n) => [`${"  ".repeat(depth)}${n.label}`, ...shape(n.children ?? [], depth + 1)]);

describe("buildChangeTree", () => {
  it("groups conflicts, changes and unversioned files (flat)", () => {
    expect(shape(buildChangeTree(status, "/r", false))).toEqual([
      "Merge Conflicts",
      "  conf.yml",
      "Changes",
      "  App.kt",
      "  AppTest.kt",
      "  Partial.kt",
      "  README.md",
      "Unversioned Files",
      "  notes.txt",
    ]);
  });

  it("groups by directory with compacted chains", () => {
    const changes = buildChangeTree(status, "/r", true).find((n) => n.group === "changes")!;
    expect(shape([changes])).toEqual([
      "Changes",
      "  src",
      "    main/kotlin",
      "      App.kt",
      "      Partial.kt",
      "    test",
      "      AppTest.kt",
      "  README.md",
    ]);
    expect(changes.children![0]!.files).toHaveLength(3);
  });
});

describe("staging area", () => {
  const shape = (nodes: ChangeNode[]): string[] =>
    nodes.flatMap((n) => [n.label, ...(n.children ?? []).map((c) => `  ${c.label}`)]);

  it("splits the tree into staged and unstaged groups", () => {
    expect(shape(buildChangeTree(status, "/r", false, undefined, [], true))).toEqual([
      "Merge Conflicts",
      "  conf.yml",
      "Staged",
      "  AppTest.kt",
      "  Partial.kt",
      "  README.md",
      "Unstaged",
      "  App.kt",
      "  Partial.kt",
      "Unversioned Files",
      "  notes.txt",
    ]);
  });

  it("gives a partially staged file a row in each group", () => {
    const roots = buildChangeTree(status, "/r", false, undefined, [], true);
    const [staged, unstaged] = [roots[1]!, roots[2]!];
    expect(staged.files.map((f) => f.relPath)).toContain("src/main/kotlin/Partial.kt");
    expect(unstaged.files.map((f) => f.relPath)).toContain("src/main/kotlin/Partial.kt");
    // Separate rows: the two entries must not share a key.
    const keys = roots.flatMap((g) => (g.children ?? []).map((c) => c.key));
    expect(new Set(keys).size).toBe(keys.length);
  });

  it("commits exactly the index, and the checkbox mirrors the staged state", () => {
    const plan = stagingPlan(status);
    expect(plan.stage).toEqual([]);
    expect(plan.unstage).toEqual([]);
    expect(plan.included.map((f) => f.relPath).sort()).toEqual([
      "README.md",
      "src/main/kotlin/Partial.kt",
      "src/test/AppTest.kt",
    ]);
    const checks = withStagedChecks(status);
    expect(checks.get("/r/notes.txt")).toBe("none");
    expect(checks.get("/r/src/main/kotlin/App.kt")).toBe("none");
    expect(checks.get("/r/README.md")).toBe("all");
    expect(checks.get("/r/src/main/kotlin/Partial.kt")).toBe("all");
    // The two rows of a partial file do not share a checkbox: only the staged group is on.
    expect(stagingNodeChecked("staged")).toBe(true);
    expect(stagingNodeChecked("unstaged")).toBe(false);
    expect(stagingNodeChecked("unversioned")).toBe(false);
    expect(stagingNodeChecked("conflicts")).toBe(false);
  });

  it("stages or unstages only the group whose checkbox was clicked", () => {
    const roots = buildChangeTree(status, "/r", false, undefined, [], true);
    const staged = roots.find((n) => n.group === "staged")!;
    const unstaged = roots.find((n) => n.group === "unstaged")!;
    const partialStaged = staged.children!.find((n) => n.label === "Partial.kt")!;
    const partialUnstaged = unstaged.children!.find((n) => n.label === "Partial.kt")!;
    expect(stagingToggle("unstaged", [partialUnstaged])).toEqual({
      stage: true,
      paths: ["/r/src/main/kotlin/Partial.kt"],
    });
    expect(stagingToggle("staged", [partialStaged])).toEqual({
      stage: false,
      paths: ["/r/src/main/kotlin/Partial.kt"],
    });
    // A mixed selection follows the clicked group, not every selected row.
    expect(stagingToggle("staged", [partialStaged, partialUnstaged]).stage).toBe(false);
    expect(stagingToggle("staged", [partialStaged, partialUnstaged]).paths).toEqual(["/r/src/main/kotlin/Partial.kt"]);
    expect(stagingToggle("conflicts", [roots[0]!])).toEqual({ stage: false, paths: [] });
  });
});

describe("several repositories", () => {
  const nested = [...status, fs("libs/core/Lib.kt", null, "modified"), fs("libs/core/new.txt", null, "untracked")];
  const repos = ["/r", "/r/libs/core"];

  it("groups each change group by repository", () => {
    expect(shape(buildChangeTree(nested, "/r", false, undefined, repos))).toEqual([
      "Merge Conflicts",
      "  r",
      "    conf.yml",
      "Changes",
      "  r",
      "    App.kt",
      "    AppTest.kt",
      "    Partial.kt",
      "    README.md",
      "  libs/core",
      "    Lib.kt",
      "Unversioned Files",
      "  r",
      "    notes.txt",
      "  libs/core",
      "    new.txt",
    ]);
    const changes = buildChangeTree(nested, "/r", true, undefined, repos)[1]!;
    expect(changes.children!.map((n) => [n.label, n.path])).toEqual([
      ["r", "/r"],
      ["libs/core", "/r/libs/core"],
    ]);
    expect(shape(changes.children![1]!.children!)).toEqual(["Lib.kt"]);
    // One repository: no repository level.
    const single = buildChangeTree(status, "/r", false, undefined, ["/r"]);
    expect(shape(single)).toEqual(shape(buildChangeTree(status, "/r", false)));
  });

  it("commits every repository with included changes, the selected one last", () => {
    const included = [nested[0]!, nested[7]!];
    expect(commitRepos(included, repos, "/r")).toEqual(["/r/libs/core", "/r"]);
    expect(commitRepos(included.slice(0, 1), repos, "/r/libs/core")).toEqual(["/r"]);
    expect(commitRepos([], repos, "/r/libs/core")).toEqual(["/r/libs/core"]);
  });

  it("routes a nested repository's own entry (submodule) to the enclosing repository", () => {
    expect(rootOf("/r/libs/core/Lib.kt", repos)).toBe("/r/libs/core");
    expect(rootOf("/r/libs/core", repos)).toBe("/r");
    expect(rootOf("/r/libs/core/", repos)).toBe("/r");
    expect(rootOf("/r", repos)).toBe("/r");
    expect(rootOf("/other", repos)).toBeNull();
  });
});

describe("ignore targets", () => {
  const group = (roots: ChangeNode[], id: string) => roots.find((n) => n.group === id)!;

  it("takes unversioned files only, not tracked changes", () => {
    const tree = buildChangeTree(status, "/r", true);
    expect(ignoreTargets([group(tree, "changes")], [])).toEqual([]);
    expect(ignoreTargets([group(tree, "unversioned")], [])).toEqual(["/r/notes.txt"]);
  });

  it("uses one rule for a directory whose files are all unversioned", () => {
    const tree = buildChangeTree(
      [
        fs("build/out.log", null, "untracked"),
        fs("build/tmp/a.tmp", null, "untracked"),
        fs("notes.txt", null, "untracked"),
      ],
      "/r",
      true,
    );
    expect(ignoreTargets([group(tree, "unversioned")], [])).toEqual(["/r/build", "/r/notes.txt"]);
  });

  it("lists the files of every repository instead of a repository root", () => {
    const nested = [...status, fs("libs/core/new.txt", null, "untracked")];
    const repos = ["/r", "/r/libs/core"];
    const tree = buildChangeTree(nested, "/r", false, undefined, repos);
    expect(ignoreTargets([group(tree, "unversioned")], repos)).toEqual(["/r/notes.txt", "/r/libs/core/new.txt"]);
  });
});

describe("commit inclusion", () => {
  it("defaults: tracked changes included, partial files keep their staged part, unversioned excluded", () => {
    expect(initialCheck(status[0]!)).toBe("all");
    expect(initialCheck(status[1]!)).toBe("staged");
    expect(initialCheck(status[4]!)).toBe("none");
    expect(initialCheck(status[5]!)).toBe("none");
  });

  it("plans staging so the index holds exactly the checked changes", () => {
    const checks = new Map<string, CheckState>([
      ["/r/README.md", "none"],
      ["/r/notes.txt", "all"],
    ]);
    const plan = planCommit(status, checks);
    expect(plan.stage.sort()).toEqual(["/r/notes.txt", "/r/src/main/kotlin/App.kt"]);
    expect(plan.unstage).toEqual(["/r/README.md"]);
    expect(plan.included.map((f) => f.relPath).sort()).toEqual([
      "notes.txt",
      "src/main/kotlin/App.kt",
      "src/main/kotlin/Partial.kt",
      "src/test/AppTest.kt",
    ]);
  });

  it("aggregates checkbox state for groups", () => {
    const checks = new Map<string, CheckState>();
    expect(aggregateCheck([status[0]!, status[3]!], checks)).toMatchObject({ checked: true, indeterminate: false });
    expect(aggregateCheck([status[0]!, status[4]!], checks)).toMatchObject({ checked: false, indeterminate: true });
    expect(aggregateCheck([status[1]!], checks)).toMatchObject({ checked: false, indeterminate: true });
  });

  it("commits a file with excluded blocks partially, without staging it", () => {
    const checks = new Map<string, CheckState>([["/r/src/main/kotlin/App.kt", "hunks"]]);
    const plan = planCommit(status, checks);
    expect(plan.partial).toEqual(["/r/src/main/kotlin/App.kt"]);
    expect(plan.stage).not.toContain("/r/src/main/kotlin/App.kt");
    expect(plan.included.map((f) => f.relPath)).toContain("src/main/kotlin/App.kt");
    expect(aggregateCheck([status[0]!], checks)).toMatchObject({ checked: false, indeterminate: true });
  });

  it("toggles single blocks: tri-state file, all in → all, all out → none", () => {
    const all = ["a", "b", "c"];
    const one = toggledHunk("all", undefined, all, "b");
    expect(one).toEqual({ check: "hunks", excluded: new Set(["b"]) });
    expect(hunkIncluded(one.check, one.excluded, "a")).toBe(true);
    expect(hunkIncluded(one.check, one.excluded, "b")).toBe(false);
    expect(toggledHunk(one.check, one.excluded, all, "b")).toEqual({ check: "all", excluded: new Set() });
    // From an unchecked file, ticking one block includes just that block.
    const picked = toggledHunk("none", undefined, all, "c");
    expect(picked).toEqual({ check: "hunks", excluded: new Set(["a", "b"]) });
    expect(toggledHunk("hunks", new Set(["a", "b"]), all, "c").check).toBe("none");
    // Keys of blocks that no longer exist are dropped.
    expect(toggledHunk("hunks", new Set(["gone", "a"]), all, "b").excluded).toEqual(new Set(["a", "b"]));
  });
});

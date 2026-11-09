import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import { getContext } from "@/platform/context";
import { ChangesTree, LocalChangesView } from "../CommitView";
import { useCommit } from "../commitStore";
import { useVcs } from "../store";

const { openInput } = vi.hoisted(() => ({ openInput: vi.fn() }));
vi.mock("@/workbench/contributions", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/workbench/contributions")>()),
  editorService: () => ({ openInput }),
}));

beforeEach(() => {
  openInput.mockClear();
  useVcs.setState({
    root: "/p",
    roots: ["/p"],
    status: [
      {
        path: "/p/documents/note.txt",
        relPath: "documents/note.txt",
        staged: null,
        unstaged: "modified",
        renamedFrom: null,
      },
    ],
  });
  useCommit.setState({ groupByDir: true, checks: new Map(), selection: [] });
});

describe.each(["commit", "local", "dialog"] as const)("%s change tree directory", (mode) => {
  it("scopes folders and file parents, and clears on groups and message focus", () => {
    render(
      <>
        <ChangesTree mode={mode} />
        <textarea aria-label="Commit message" />
      </>,
    );
    const tree = screen.getByRole("tree", { name: "Changes" });
    tree.focus();
    fireEvent.keyDown(tree, { key: "Home" });
    expect(getContext().focusedDirectory).toBeUndefined();
    fireEvent.keyDown(tree, { key: "ArrowDown" });
    expect(getContext().focusedDirectory).toBe("/p/documents");
    fireEvent.keyDown(tree, { key: "ArrowRight" });
    fireEvent.keyDown(tree, { key: "ArrowDown" });
    expect(getContext().focusedDirectory).toBe("/p/documents");
    if (mode === "commit") {
      expect(openInput).toHaveBeenLastCalledWith(
        expect.objectContaining({ kind: "diff", path: "/p/documents/note.txt" }),
        { preview: true, focus: false },
      );
    }
    screen.getByLabelText("Commit message").focus();
    expect(getContext().focusedDirectory).toBeUndefined();
  });
});

it("groups files under a compact directory and uses row actions in the commit tree", () => {
  useCommit.setState({ groupByDir: false });
  const path = "/p/packages/mobile/src/Main.vue";
  const files = [path, "/p/packages/mobile/src/Panel.vue"];
  useVcs.setState({
    status: files.map((file) => ({
      path: file,
      relPath: file.slice(3),
      staged: null,
      unstaged: "modified",
      renamedFrom: null,
    })),
  });
  render(<ChangesTree mode="commit" />);
  expect(screen.getByText("packages/mobile/src")).toBeInTheDocument();
  expect(screen.queryByRole("checkbox")).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "Exclude from Commit · Main.vue" }));
  expect(useCommit.getState().checks.get(path)).toBe("none");
  fireEvent.click(screen.getByRole("button", { name: "Include in Commit · Main.vue" }));
  expect(useCommit.getState().checks.get(path)).toBe("all");
});

it("opens a pinned merge editor when selecting a conflicted file in the commit tree", () => {
  const path = "/p/conflict.txt";
  useVcs.setState({
    status: [{ path, relPath: "conflict.txt", staged: null, unstaged: "conflicted", renamedFrom: null }],
  });
  render(<ChangesTree mode="commit" />);
  const row = screen.getByText("conflict.txt");
  fireEvent.mouseDown(row);
  expect(openInput).toHaveBeenLastCalledWith(expect.objectContaining({ kind: "merge", path }), { focus: false });
  fireEvent.doubleClick(row);
  expect(openInput).toHaveBeenLastCalledWith(expect.objectContaining({ kind: "merge", path }), { focus: true });
});

it("offers to resolve a conflicted file from the local changes preview", () => {
  const path = "/p/conflict.txt";
  const file = { path, relPath: "conflict.txt", staged: null, unstaged: "conflicted" as const, renamedFrom: null };
  useVcs.setState({ status: [file], byPath: new Map([[path, file]]) });
  useCommit.setState({ diffPreview: true });
  render(<LocalChangesView />);
  fireEvent.mouseDown(screen.getByText("conflict.txt"));
  expect(openInput).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Resolve conflicts…" }));
  expect(openInput).toHaveBeenLastCalledWith(expect.objectContaining({ kind: "merge", path }), { focus: true });
});

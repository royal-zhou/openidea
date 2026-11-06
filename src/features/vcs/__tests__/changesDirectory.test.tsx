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
    screen.getByLabelText("Commit message").focus();
    expect(getContext().focusedDirectory).toBeUndefined();
  });
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

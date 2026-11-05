import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import type { commands } from "@/platform/ipc";

const { lspGoto, fsResolveModule, webGoto } = vi.hoisted(() => ({
  lspGoto: vi.fn<typeof commands.lspGoto>(),
  fsResolveModule: vi.fn<typeof commands.fsResolveModule>(),
  webGoto: vi.fn<typeof commands.webGoto>(),
}));
vi.mock("@/platform/ipc", async (orig) => {
  const real = await orig<typeof import("@/platform/ipc")>();
  return { ...real, isTauri: () => true, commands: { ...real.commands, lspGoto, fsResolveModule, webGoto } };
});

const { gotoTargets } = await import("./navigation");
const { lspDoc } = await import("./lsp");
const { provideEditorService } = await import("@/workbench/contributions");
const openFile = vi.fn(async () => {});
provideEditorService({ openFile } as never);

let view: EditorView;
beforeEach(() => {
  vi.useFakeTimers();
  openFile.mockClear();
  lspGoto.mockReset();
  view = new EditorView({
    state: EditorState.create({
      doc: "run()",
      extensions: [lspDoc.of({ doc: 1, path: "/p/main.rs", language: "rust", uiMode: "Rust", readOnly: false })],
    }),
    parent: document.body,
  });
});
afterEach(() => {
  view.destroy();
  vi.useRealTimers();
});

describe("slow navigation feedback", () => {
  const target = {
    path: "/p/packages/core/src/office.ts",
    range: { start: { line: 3, col: 4 }, end: { line: 3, col: 19 } },
    uri: null,
  };

  function packageImport() {
    view.destroy();
    const doc = "import { useOfficeDetail } from '@tongban/sceneCore';";
    view = new EditorView({
      state: EditorState.create({
        doc,
        selection: { anchor: doc.indexOf("useOfficeDetail") },
        extensions: [lspDoc.of({ doc: 2, path: "/p/App.vue", language: "vue", uiMode: "Vue", readOnly: false })],
      }),
      parent: document.body,
    });
  }

  it("retries an empty Vue package binding and jumps when analysis resolves it", async () => {
    packageImport();
    lspGoto.mockResolvedValueOnce({ status: "ok", data: [] }).mockResolvedValueOnce({ status: "ok", data: [target] });
    const pending = gotoTargets(view, "definition");
    await vi.advanceTimersByTimeAsync(250);
    expect(document.querySelector(".cm-lsp-hint")?.textContent).toBe("Analyzing…");
    expect(openFile).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(250);
    await pending;
    expect(openFile).toHaveBeenCalledWith(target.path, { selection: target.range.start, focus: true });
    expect(document.querySelector(".cm-lsp-hint")).toBeNull();
  });

  it("drops a pending jump after the caret moves", async () => {
    packageImport();
    lspGoto.mockResolvedValueOnce({ status: "ok", data: [] });
    const pending = gotoTargets(view, "definition");
    await vi.advanceTimersByTimeAsync(250);
    view.dispatch({ selection: { anchor: 0 } });
    await vi.advanceTimersByTimeAsync(250);
    await pending;
    expect(lspGoto).toHaveBeenCalledTimes(1);
    expect(openFile).not.toHaveBeenCalled();
  });

  it("opens the Vue component when go to definition returns the import binding itself", async () => {
    view.destroy();
    const path = "/p/pages/oneThing/TSceneOfficeDetail.vue";
    const specifier = "../../businessComponents/tSceneOfficeDetail/OfficeBanner";
    const component = "/p/businessComponents/tSceneOfficeDetail/OfficeBanner.vue";
    const doc = `import OfficeBanner from '${specifier}';`;
    view = new EditorView({
      state: EditorState.create({
        doc,
        selection: { anchor: doc.indexOf("OfficeBanner") + 1 },
        extensions: [lspDoc.of({ doc: 2, path, language: "vue", uiMode: "Vue", readOnly: false })],
      }),
      parent: document.body,
    });
    lspGoto.mockResolvedValueOnce({
      status: "ok",
      data: [{ path, range: { start: { line: 0, col: 7 }, end: { line: 0, col: 19 } }, uri: null }],
    });
    webGoto.mockResolvedValueOnce({ status: "ok", data: null });
    fsResolveModule.mockResolvedValueOnce({
      status: "ok",
      data: [{ path: component, dir: false, via: "relative" }],
    });

    await gotoTargets(view, "definition");

    expect(fsResolveModule).toHaveBeenCalledWith(path, specifier);
    expect(openFile).toHaveBeenCalledWith(component, { selection: { line: 0, col: 0 }, focus: true });
  });

  it("shows progress while finding a definition, then opens the target and clears the hint", async () => {
    let resolve!: (result: Awaited<ReturnType<typeof commands.lspGoto>>) => void;
    lspGoto.mockImplementationOnce(() => new Promise((r) => (resolve = r)));
    const pending = gotoTargets(view, "definition");
    await vi.advanceTimersByTimeAsync(250);
    expect(document.querySelector(".cm-lsp-hint")?.textContent).toBe("Finding navigation target…");
    expect(openFile).not.toHaveBeenCalled();
    resolve({
      status: "ok",
      data: [{ path: "/p/lib.rs", range: { start: { line: 3, col: 4 }, end: { line: 3, col: 7 } }, uri: null }],
    });
    await pending;
    expect(openFile).toHaveBeenCalledWith("/p/lib.rs", { selection: { line: 3, col: 4 }, focus: true });
    expect(document.querySelector(".cm-lsp-hint")).toBeNull();
  });

  it("replaces progress with the server error", async () => {
    let resolve!: (result: Awaited<ReturnType<typeof commands.lspGoto>>) => void;
    lspGoto.mockImplementationOnce(() => new Promise((r) => (resolve = r)));
    const pending = gotoTargets(view, "implementation");
    await vi.advanceTimersByTimeAsync(250);
    resolve({ status: "error", error: { kind: "protocol", message: "Server timed out" } });
    await pending;
    expect(document.querySelector(".cm-lsp-hint")?.textContent).toBe("Server timed out");
    expect(openFile).not.toHaveBeenCalled();
  });
});

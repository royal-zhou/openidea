import type { Terminal } from "@xterm/xterm";

type RowRenderer = { renderRows(start: number, end: number): void };

export function protectSynchronizedOutput(term: Terminal) {
  // xterm 6.0's DOM selection redraw calls renderRows directly, bypassing the
  // render service's DEC 2026 guard. The service repaints when the frame ends.
  const { _core } = term as Terminal & {
    _core?: { _renderService?: { _renderer?: { value?: RowRenderer } } };
  };
  const renderer = _core?._renderService?._renderer?.value;
  if (typeof renderer?.renderRows !== "function") return;
  const renderRows = renderer.renderRows.bind(renderer);
  renderer.renderRows = (start, end) => {
    if (!term.modes.synchronizedOutputMode) renderRows(start, end);
  };
}

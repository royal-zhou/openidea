import type { IDisposable, Terminal } from "@xterm/xterm";

export function trackScrollbackErase(term: Terminal): IDisposable[] {
  // xterm 6.0 leaves the viewport detached after ED3 erases the history it was showing.
  // Reattach before xterm handles the erase. https://github.com/xtermjs/xterm.js/pull/6081
  return [undefined, "?"].map((prefix) =>
    term.parser.registerCsiHandler({ prefix, final: "J" }, (params) => {
      const buffer = term.buffer.active;
      if (params[0] === 3 && buffer.type === "normal" && buffer.viewportY < buffer.baseY) {
        term.scrollToBottom();
      }
      return false;
    }),
  );
}

import { expect, test } from "@playwright/test";

test("terminal follows new output after a synchronized redraw clears the history", async ({ page }) => {
  await page.goto("/");
  const positions = await page.evaluate(async () => {
    const sessionPath = "/src/features/terminal/session.ts";
    const { TerminalSession } = await import(sessionPath);
    const container = document.createElement("div");
    container.style.cssText = "position:fixed;inset:0;z-index:99999";
    document.body.append(container);
    const session = new TerminalSession({ key: "scroll", cwd: "/", onChange() {}, onFocus() {} });
    const term = session.term;
    const transcript = (label: string) =>
      Array.from({ length: 200 }, (_, index) => `${label} ${index + 1}`).join("\r\n");
    const write = (data: string) => new Promise<void>((resolve) => term.write(data, resolve));
    const frame = () => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
    const atBottom = () => term.buffer.active.viewportY === term.buffer.active.baseY;
    try {
      session.attach(container);
      await session.start();
      await write(transcript("Original output"));
      await frame();
      term.scrollLines(-10);
      const beforeRedraw = atBottom();

      // Pi replaces its transcript in a single synchronized update.
      await write("\x1b[?2026h\x1b[2J\x1b[H\x1b[3J" + transcript("Redrawn output") + "\x1b[?2026l");
      await write("\x1b[?2026h\r\nLatest streamed output\x1b[?2026l");
      await frame();
      const afterRedraw = atBottom();

      term.scrollLines(-10);
      await write("\r\nMore output");
      await frame();
      const whileReadingHistory = atBottom();
      term.scrollLines(term.buffer.active.length);
      await frame();
      return { beforeRedraw, afterRedraw, whileReadingHistory, afterScrollDown: atBottom() };
    } finally {
      session.dispose();
      container.remove();
    }
  });
  expect(positions).toEqual({
    beforeRedraw: false,
    afterRedraw: true,
    whileReadingHistory: false,
    afterScrollDown: true,
  });
});

test("a short terminal keeps the last complete frame while Pi redraws", async ({ page }) => {
  await page.goto("/");
  const result = await page.evaluate(async () => {
    const sessionPath = "/src/features/terminal/session.ts";
    const { TerminalSession } = await import(sessionPath);
    const container = document.createElement("div");
    container.style.cssText = "position:fixed;inset:auto 0 0;height:108px;z-index:99999";
    document.body.append(container);
    const session = new TerminalSession({ key: "short-terminal", cwd: "/", onChange() {}, onFocus() {} });
    const term = session.term;
    const write = (data: string) => new Promise<void>((resolve) => term.write(data, resolve));
    const frame = () => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
    const rendered = () => container.querySelector(".xterm-rows")?.textContent;
    try {
      session.attach(container);
      await session.start();
      await write("Old response\r\n> Previous prompt");
      await frame();
      const before = rendered();

      // A full redraw can be split across PTY reads. The next prompt is in the last read.
      await write("\x1b[?2026h\x1b[2J\x1b[H\x1b[3J" + "Intermediate response line\r\n".repeat(24));
      await frame();
      const during = rendered();
      const synchronized = term.modes.synchronizedOutputMode;
      await write("> New prompt\x1b[?2026l");
      await frame();
      return { rows: term.rows, before, during, synchronized, after: rendered() };
    } finally {
      session.dispose();
      container.remove();
    }
  });

  expect(result.rows).toBeGreaterThan(2);
  expect(result.rows).toBeLessThanOrEqual(8);
  expect(result.before).toContain("> Previous prompt");
  expect(result.synchronized).toBe(true);
  expect(result.during).toBe(result.before);
  expect(result.after).toContain("> New prompt");
});

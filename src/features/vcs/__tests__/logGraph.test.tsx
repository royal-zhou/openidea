import { describe, expect, it } from "vitest";
import { render } from "@testing-library/react";
import { GraphCell } from "../GitLog";
import { computeGraph, laneCount } from "../graph";

const c = (id: string, ...parents: string[]) => ({ id, parents });

const svgWidth = (el: Element) => Number(el.querySelector("svg")?.getAttribute("width"));

describe("Git log graph column", () => {
  it("gives every row the same width, so subjects line up", () => {
    // t's parent p is two rows down: t's row is one lane wide, the ones in between two.
    const rows = computeGraph([c("t", "p"), c("x"), c("p")]);
    expect(rows.map((r) => r.width)).toEqual([1, 2, 1]);
    const lanes = laneCount(rows);
    expect(lanes).toBe(2);
    const { container } = render(
      <>
        {rows.map((r, i) => (
          <GraphCell key={i} row={r} lanes={lanes} />
        ))}
      </>,
    );
    const widths = new Set([...container.querySelectorAll("svg")].map((s) => s.getAttribute("width")));
    expect(widths.size).toBe(1);
  });

  it("keeps a filtered history to one lane", () => {
    // A file's history: the parents are not rows of the log.
    const rows = computeGraph([c("d", "w"), c("c", "x"), c("b", "y"), c("a", "z")]);
    expect(laneCount(rows)).toBe(1);
    const { container: narrow } = render(<GraphCell row={rows[0]!} lanes={1} />);
    const { container: wide } = render(<GraphCell row={rows[0]!} lanes={2} />);
    expect(svgWidth(narrow)).toBeLessThan(svgWidth(wide));
  });
});

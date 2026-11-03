import { describe, expect, it } from "vitest";
import { computeGraph, laneCount } from "../graph";

const c = (id: string, ...parents: string[]) => ({ id, parents });

describe("computeGraph", () => {
  it("keeps a linear history in lane 0", () => {
    const rows = computeGraph([c("c", "b"), c("b", "a"), c("a")]);
    expect(rows.map((r) => r.lane)).toEqual([0, 0, 0]);
    expect(rows[0]!.up).toEqual([]);
    expect(rows[0]!.down).toEqual([{ from: 0, to: 0, color: 0 }]);
    expect(rows[2]!.down).toEqual([]);
    expect(new Set(rows.map((r) => r.color)).size).toBe(1);
    expect(laneCount(rows)).toBe(1);
  });

  it("opens a lane for a merge's second parent and joins it back", () => {
    // m merges f into main; f branched from a.
    const rows = computeGraph([c("m", "b", "f"), c("f", "a"), c("b", "a"), c("a")]);
    const [m, f, b, a] = rows;
    expect(m!.lane).toBe(0);
    expect(m!.down).toEqual([
      { from: 0, to: 0, color: m!.color },
      { from: 0, to: 1, color: f!.color },
    ]);
    expect(f!.lane).toBe(1);
    expect(f!.color).not.toBe(m!.color);
    // b passes lane 1 (f's parent a) through while sitting in lane 0.
    expect(b!.lane).toBe(0);
    expect(b!.down).toContainEqual({ from: 1, to: 1, color: f!.color });
    // a is expected by both lanes: both lines converge into its node.
    expect(a!.lane).toBe(0);
    expect(a!.up).toEqual([
      { from: 0, to: 0, color: m!.color },
      { from: 1, to: 0, color: f!.color },
    ]);
    expect(a!.width).toBe(2);
    expect(laneCount(rows)).toBe(2);
  });

  it("gives no lane to a parent the log does not show", () => {
    // A file's history: consecutive commits touching the file are rarely parent and
    // child, so their parents are not rows of the log.
    const rows = computeGraph([c("d", "w"), c("c", "x"), c("b", "y"), c("a", "z")]);
    expect(rows.map((r) => r.lane)).toEqual([0, 0, 0, 0]);
    expect(rows.map((r) => r.down)).toEqual([[], [], [], []]);
    expect(laneCount(rows)).toBe(1);
  });

  it("keeps a lane for a parent shown further down the log", () => {
    const rows = computeGraph([c("t", "p"), c("x"), c("y"), c("p")]);
    expect(rows[0]!.down).toEqual([{ from: 0, to: 0, color: rows[0]!.color }]);
    expect(rows[1]!.lane).toBe(1);
    expect(rows[1]!.up).toEqual([{ from: 0, to: 0, color: rows[0]!.color }]);
    expect(rows[3]!.lane).toBe(0);
    expect(rows[3]!.down).toEqual([]);
  });

  it("does not wait for a parent listed above the commit", () => {
    // Out-of-order pages (clock skew, a merge right below its tip): no lane is left open.
    const rows = computeGraph([c("b"), c("a", "b"), c("z", "a")]);
    expect(rows.map((r) => r.down)).toEqual([[], [], []]);
    expect(laneCount(rows)).toBe(1);
  });

  it("converges lanes that lead to the same parent at its node", () => {
    const rows = computeGraph([c("t1", "p"), c("t2", "p"), c("p")]);
    expect(rows[1]!.lane).toBe(1);
    expect(rows[1]!.down).toContainEqual({ from: 1, to: 1, color: rows[1]!.color });
    expect(rows[2]!.lane).toBe(0);
    expect(rows[2]!.up).toEqual([
      { from: 0, to: 0, color: rows[0]!.color },
      { from: 1, to: 0, color: rows[1]!.color },
    ]);
  });

  it("joins a merge parent that is already on another lane", () => {
    // m2 merges x, which lane 1 (from t) already leads to.
    const rows = computeGraph([c("t", "x"), c("m2", "b", "x"), c("x", "b"), c("b")]);
    const m2 = rows[1]!;
    expect(m2.down).toContainEqual({ from: m2.lane, to: 0, color: rows[0]!.color });
  });
});

import { describe, expect, test } from "vitest";

import { Plan } from "./plan";
import { emptyFilter } from "./view";

/** Walk every index and describe what the plan puts there. */
function shape(plan: Plan): string[] {
  const out: string[] = [];
  for (let i = 0; i < plan.total; i += 1) {
    const item = plan.at(i);
    out.push(item === null ? "null" : `${item.block.key}[${item.offset}]`);
  }
  return out;
}

describe("the timeline plan", () => {
  test("is one block, one row per index", () => {
    const plan = Plan.flat(emptyFilter(), 3);
    expect(plan.total).toBe(3);
    expect(shape(plan)).toEqual(["all[0]", "all[1]", "all[2]"]);
    expect(plan.at(3)).toBeNull();
    expect(plan.at(-1)).toBeNull();
  });

  test("an empty result has no items at all", () => {
    expect(Plan.flat(emptyFilter(), 0).total).toBe(0);
  });

  test("index lookup is stable across a large plan", () => {
    // The list is virtualized over the whole result, so a jump into the middle
    // of 100k rows has to land on the right one without walking there.
    const plan = Plan.flat(emptyFilter(), 100_000);
    expect(plan.total).toBe(100_000);
    expect(plan.at(0)).toMatchObject({ offset: 0 });
    expect(plan.at(99_999)).toMatchObject({ offset: 99_999 });
    expect(plan.at(100_000)).toBeNull();
  });
});

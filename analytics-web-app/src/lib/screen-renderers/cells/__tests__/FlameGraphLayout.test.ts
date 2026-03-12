import { computeAsyncVisualDepths, type SpanData } from '../FlameGraphCell'

describe('computeAsyncVisualDepths', () => {
  it('places children directly below their parent in a chain', () => {
    // Reproduces the bug where BFS layout groups all spans at the same depth
    // together, separating children from their parents.
    //
    // Tree structure (two parallel chains under one root):
    //   write (depth=5, t=0..100)
    //     ├─ process_A (depth=6, t=10..50)
    //     │   └─ for_each_A (depth=7, t=15..45)
    //     │       └─ fetch_A (depth=8, t=20..40)
    //     └─ process_B (depth=6, t=60..95)
    //         └─ for_each_B (depth=7, t=65..90)
    //             └─ fetch_B (depth=8, t=70..85)
    //
    // Correct DFS layout (children directly below parent):
    //   vd=0: write
    //   vd=1: process_A
    //   vd=2: for_each_A
    //   vd=3: fetch_A
    //   vd=4: process_B
    //   vd=5: for_each_B
    //   vd=6: fetch_B
    //
    // BFS bug layout (all depth-6 together, then depth-7, then depth-8):
    //   vd=0: write
    //   vd=1: process_A
    //   vd=2: process_B       <-- B is NOT a sibling of A's children
    //   vd=3: for_each_A
    //   vd=4: for_each_B
    //   vd=5: fetch_A
    //   vd=6: fetch_B

    const spans: SpanData[] = [
      { id: 1, parent: 0, begin: 0,  end: 100, depth: 5 }, // write
      { id: 2, parent: 1, begin: 10, end: 50,  depth: 6 }, // process_A
      { id: 3, parent: 2, begin: 15, end: 45,  depth: 7 }, // for_each_A
      { id: 4, parent: 3, begin: 20, end: 40,  depth: 8 }, // fetch_A
      { id: 5, parent: 1, begin: 60, end: 95,  depth: 6 }, // process_B
      { id: 6, parent: 5, begin: 65, end: 90,  depth: 7 }, // for_each_B
      { id: 7, parent: 6, begin: 70, end: 85,  depth: 8 }, // fetch_B
    ]

    const vd = computeAsyncVisualDepths(spans)

    // write is at the top
    expect(vd[0]).toBe(0) // write

    // process_A is directly below write
    expect(vd[1]).toBe(vd[0] + 1) // process_A = write + 1

    // for_each_A is directly below process_A
    expect(vd[2]).toBe(vd[1] + 1) // for_each_A = process_A + 1

    // fetch_A is directly below for_each_A
    expect(vd[3]).toBe(vd[2] + 1) // fetch_A = for_each_A + 1

    // process_B is below fetch_A (not between process_A and for_each_A)
    expect(vd[4]).toBeGreaterThan(vd[3]) // process_B after fetch_A's chain

    // for_each_B is directly below process_B
    expect(vd[5]).toBe(vd[4] + 1) // for_each_B = process_B + 1

    // fetch_B is directly below for_each_B
    expect(vd[6]).toBe(vd[5] + 1) // fetch_B = for_each_B + 1
  })

  it('places sequential siblings on separate rows to show tree structure', () => {
    // Even non-overlapping siblings get their own subtree row so that
    // each child chain is visually distinct below the parent.
    //   root (t=0..100)
    //     ├─ child_A (t=10..40)
    //     └─ child_B (t=50..90)

    const spans: SpanData[] = [
      { id: 1, parent: 0, begin: 0,  end: 100, depth: 0 },
      { id: 2, parent: 1, begin: 10, end: 40,  depth: 1 },
      { id: 3, parent: 1, begin: 50, end: 90,  depth: 1 },
    ]

    const vd = computeAsyncVisualDepths(spans)
    expect(vd[0]).toBe(0)
    expect(vd[1]).toBe(1) // child_A directly below root
    expect(vd[2]).toBe(2) // child_B below child_A's subtree
  })

  it('stacks overlapping siblings on separate rows', () => {
    // Two concurrent children must not overlap visually
    //   root (t=0..100)
    //     ├─ child_A (t=10..60)
    //     └─ child_B (t=30..90)

    const spans: SpanData[] = [
      { id: 1, parent: 0, begin: 0,  end: 100, depth: 0 },
      { id: 2, parent: 1, begin: 10, end: 60,  depth: 1 },
      { id: 3, parent: 1, begin: 30, end: 90,  depth: 1 },
    ]

    const vd = computeAsyncVisualDepths(spans)
    expect(vd[0]).toBe(0)
    expect(vd[1]).toBe(1)
    expect(vd[2]).toBe(2) // bumped because it overlaps child_A
  })
})

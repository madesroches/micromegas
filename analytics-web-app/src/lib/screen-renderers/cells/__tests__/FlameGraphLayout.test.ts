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
    // Correct layout — non-overlapping chains share rows:
    //   vd=0: write
    //   vd=1: process_A, process_B   (same row, non-overlapping)
    //   vd=2: for_each_A, for_each_B (same row, non-overlapping)
    //   vd=3: fetch_A, fetch_B       (same row, non-overlapping)
    //
    // BFS bug layout (all depth-6 together, then depth-7, then depth-8):
    //   vd=0: write
    //   vd=1: process_A, process_B
    //   vd=2: for_each_A, for_each_B  <-- looks same but only by accident
    //   vd=3: fetch_A, fetch_B
    //
    // The BFS bug shows up when chains overlap — see "overlapping chains" test.

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

    // Both process spans share a row directly below write
    expect(vd[1]).toBe(1) // process_A
    expect(vd[4]).toBe(1) // process_B (non-overlapping, shares row)

    // Both for_each spans share the next row
    expect(vd[2]).toBe(2) // for_each_A
    expect(vd[5]).toBe(2) // for_each_B

    // Both fetch spans share the next row
    expect(vd[3]).toBe(3) // fetch_A
    expect(vd[6]).toBe(3) // fetch_B
  })

  it('packs non-overlapping siblings on the same row', () => {
    // Sequential children that don't overlap should share a visual row.
    // This is the common case: a loop calling materialize_partition_range
    // repeatedly — each call should be on the same row as the previous.
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
    expect(vd[2]).toBe(1) // child_B shares row with child_A (non-overlapping)
  })

  it('packs non-overlapping sibling chains on shared rows', () => {
    // Sequential parent-child chains should share visual rows when
    // they don't overlap in time. This is the materialize_partition_range
    // pattern: each call and its children are sequential.
    //
    //   root (t=0..200)
    //     ├─ batch_A (t=10..80)
    //     │   └─ query_A (t=20..70)
    //     └─ batch_B (t=100..180)
    //         └─ query_B (t=110..170)
    //
    // Expected layout (non-overlapping chains share rows):
    //   vd=0: root
    //   vd=1: batch_A, batch_B
    //   vd=2: query_A, query_B

    const spans: SpanData[] = [
      { id: 1, parent: 0, begin: 0,   end: 200, depth: 0 },
      { id: 2, parent: 1, begin: 10,  end: 80,  depth: 1 },
      { id: 3, parent: 2, begin: 20,  end: 70,  depth: 2 },
      { id: 4, parent: 1, begin: 100, end: 180, depth: 1 },
      { id: 5, parent: 4, begin: 110, end: 170, depth: 2 },
    ]

    const vd = computeAsyncVisualDepths(spans)
    expect(vd[0]).toBe(0)
    expect(vd[1]).toBe(1) // batch_A
    expect(vd[2]).toBe(2) // query_A directly below batch_A
    expect(vd[3]).toBe(1) // batch_B shares row with batch_A
    expect(vd[4]).toBe(2) // query_B shares row with query_A
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

  it('keeps overlapping chains children below their own parent', () => {
    // Two concurrent parent-child chains — children must stay below
    // their respective parent, not get grouped by depth level.
    //
    //   root (t=0..200)
    //     ├─ parent_A (t=10..100)
    //     │   └─ child_A (t=20..90)
    //     └─ parent_B (t=30..150)    <-- overlaps parent_A
    //         └─ child_B (t=40..140)
    //
    // Correct DFS layout:
    //   vd=0: root
    //   vd=1: parent_A
    //   vd=2: child_A        <-- directly below parent_A
    //   vd=3: parent_B       <-- bumped because it overlaps parent_A
    //   vd=4: child_B        <-- directly below parent_B
    //
    // BFS bug layout would put both parents at vd=1,2 then both children at vd=3,4:
    //   vd=0: root
    //   vd=1: parent_A
    //   vd=2: parent_B
    //   vd=3: child_A         <-- NOT directly below parent_A
    //   vd=4: child_B

    const spans: SpanData[] = [
      { id: 1, parent: 0, begin: 0,   end: 200, depth: 0 },
      { id: 2, parent: 1, begin: 10,  end: 100, depth: 1 },
      { id: 3, parent: 2, begin: 20,  end: 90,  depth: 2 },
      { id: 4, parent: 1, begin: 30,  end: 150, depth: 1 },
      { id: 5, parent: 4, begin: 40,  end: 140, depth: 2 },
    ]

    const vd = computeAsyncVisualDepths(spans)
    expect(vd[0]).toBe(0)                // root
    expect(vd[1]).toBe(1)                // parent_A
    expect(vd[2]).toBe(vd[1] + 1)       // child_A directly below parent_A
    expect(vd[3]).toBeGreaterThan(vd[2]) // parent_B below child_A (overlap)
    expect(vd[4]).toBe(vd[3] + 1)       // child_B directly below parent_B
  })
})

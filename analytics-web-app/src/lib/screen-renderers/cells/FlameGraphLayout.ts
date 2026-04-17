export interface SpanData {
  id: number
  parent: number
  begin: number
  end: number
  depth: number
}

/**
 * Compute visual depths for async spans using DFS tree-walk layout.
 * Children are placed directly below their parent; concurrent siblings
 * get bumped to the next visual row.
 * Returns an array of visual depths, one per input span (in input order).
 */
export function computeAsyncVisualDepths(spans: SpanData[]): number[] {
  const n = spans.length
  if (n === 0) return []

  // Build id → index lookup
  const idToIdx = new Map<number, number>()
  for (let i = 0; i < n; i++) {
    idToIdx.set(spans[i].id, i)
  }

  // Build children map and find roots
  const childrenOf = new Map<number, number[]>()
  const roots: number[] = []
  for (let i = 0; i < n; i++) {
    const parentIdx = idToIdx.get(spans[i].parent)
    if (parentIdx != null && parentIdx !== i) {
      if (!childrenOf.has(spans[i].parent)) childrenOf.set(spans[i].parent, [])
      childrenOf.get(spans[i].parent)!.push(i)
    } else {
      roots.push(i)
    }
  }

  // Collect subtrees
  interface SubTree {
    members: number[]
    minBegin: number
    maxEnd: number
  }
  const visited = new Uint8Array(n)
  const trees: SubTree[] = []

  for (const rootIdx of roots) {
    const members: number[] = []
    let minBegin = Infinity
    let maxEnd = -Infinity
    const stack: number[] = [rootIdx]

    while (stack.length > 0) {
      const idx = stack.pop()!
      if (visited[idx]) continue
      visited[idx] = 1
      members.push(idx)

      if (spans[idx].begin < minBegin) minBegin = spans[idx].begin
      if (spans[idx].end > maxEnd) maxEnd = spans[idx].end

      const children = childrenOf.get(spans[idx].id)
      if (children) {
        for (let c = children.length - 1; c >= 0; c--) {
          stack.push(children[c])
        }
      }
    }

    trees.push({ members, minBegin, maxEnd })
  }

  // Layout each subtree using DFS order + row-end tracking.
  // DFS ensures children are processed right after their parent.
  // Row-end tracking allows non-overlapping siblings to share visual rows.
  const globalRowEnds: number[] = []
  const visualDepths = new Array<number>(n).fill(0)

  for (const tree of trees) {
    const memberRelVd = new Map<number, number>()
    let treeHeight = 0

    // Find roots within this subtree
    const treeRoots = tree.members.filter((idx) => {
      const parentIdx = idToIdx.get(spans[idx].parent)
      return parentIdx == null || parentIdx === idx
    })

    // DFS stack: process each node, then its children immediately after
    const dfsStack: { idx: number; parentVd: number }[] = []
    for (let i = treeRoots.length - 1; i >= 0; i--) {
      dfsStack.push({ idx: treeRoots[i], parentVd: -1 })
    }

    // Track end time per visual row — non-overlapping spans reuse the same row
    const vdRowEnds = new Map<number, number>()

    while (dfsStack.length > 0) {
      const { idx, parentVd } = dfsStack.pop()!
      const s = spans[idx]
      const baseVd = parentVd + 1

      // Find first available visual row at baseVd or deeper
      let vd = baseVd
      for (;;) {
        const endTime = vdRowEnds.get(vd)
        if (endTime == null || s.begin >= endTime) {
          vdRowEnds.set(vd, s.end)
          break
        }
        vd++
      }

      memberRelVd.set(idx, vd)
      if (vd + 1 > treeHeight) treeHeight = vd + 1

      // Push children in reverse-begin order so earliest is popped first
      const children = childrenOf.get(s.id)
      if (children) {
        const sorted = [...children].sort((a, b) => spans[b].begin - spans[a].begin)
        for (const childIdx of sorted) {
          dfsStack.push({ idx: childIdx, parentVd: vd })
        }
      }
    }

    // Find lowest global base where the tree block fits
    let base = 0
    let searching = true
    while (searching) {
      searching = false
      for (let d = 0; d < treeHeight; d++) {
        const r = base + d
        if (r < globalRowEnds.length && globalRowEnds[r] > tree.minBegin) {
          base = r + 1
          searching = true
          break
        }
      }
    }

    // Assign global visual depths
    for (const idx of tree.members) {
      visualDepths[idx] = base + memberRelVd.get(idx)!
    }

    // Reserve the tree block
    for (let d = 0; d < treeHeight; d++) {
      const r = base + d
      while (globalRowEnds.length <= r) globalRowEnds.push(0)
      if (tree.maxEnd > globalRowEnds[r]) globalRowEnds[r] = tree.maxEnd
    }
  }

  return visualDepths
}

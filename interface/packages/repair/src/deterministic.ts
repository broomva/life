import type { UINode, Violation, RepairStrategy } from '@life/ikr-ir';

/**
 * Apply deterministic repair strategies that require no AI.
 * Returns a modified copy of the spec (immutable — never mutates input).
 */
export function applyDeterministicRepairs(
  spec: UINode,
  violations: Violation[],
): { repaired: UINode; applied: string[]; remaining: Violation[] } {
  let current = structuredClone(spec);
  const applied: string[] = [];
  const remaining: Violation[] = [];

  for (const violation of violations) {
    const strategy = findDeterministicStrategy(violation.repairOptions);
    if (strategy) {
      current = applyStrategy(current, strategy);
      applied.push(`${violation.rule}:${strategy.kind}`);
    } else {
      remaining.push(violation);
    }
  }

  return { repaired: current, applied, remaining };
}

function findDeterministicStrategy(
  options: RepairStrategy[],
): RepairStrategy | null {
  // Prefer deterministic strategies over AI-requiring ones
  const deterministicKinds = new Set([
    'collapse_chips',
    'switch_density',
    'widen_container',
    'increase_max_lines',
    'hide_node',
  ]);
  return options.find((o) => deterministicKinds.has(o.kind)) ?? null;
}

function applyStrategy(spec: UINode, strategy: RepairStrategy): UINode {
  switch (strategy.kind) {
    case 'collapse_chips':
      return collapseChips(spec, strategy.nodeId, strategy.maxVisible);
    case 'switch_density':
      return switchDensity(spec, strategy.nodeId, strategy.density);
    case 'hide_node':
      return hideNode(spec, strategy.nodeId);
    case 'increase_max_lines':
      return increaseMaxLines(spec, strategy.nodeId, strategy.lines);
    case 'widen_container':
      // Cannot widen without knowing parent constraints — skip
      return spec;
    default:
      return spec;
  }
}

function collapseChips(
  spec: UINode,
  nodeId: string,
  maxVisible: number,
): UINode {
  return mapNode(spec, (node) => {
    if (node.id !== nodeId || node.kind !== 'inlineRow') return node;
    const visibleChildren = node.children.slice(0, maxVisible);
    const hiddenCount = node.children.length - maxVisible;
    if (hiddenCount > 0) {
      visibleChildren.push({
        kind: 'chip',
        id: `${nodeId}-overflow`,
        label: `+${hiddenCount}`,
        priority: 0,
      });
    }
    return { ...node, children: visibleChildren };
  });
}

function switchDensity(
  spec: UINode,
  nodeId: string,
  density: 'compact',
): UINode {
  return mapNode(spec, (node) => {
    if (node.id !== nodeId) return node;
    if (node.kind === 'card') {
      return {
        ...node,
        padding:
          density === 'compact'
            ? Math.floor(node.padding / 2)
            : node.padding,
        constraints: { ...node.constraints, density },
      };
    }
    return node;
  });
}

function hideNode(spec: UINode, nodeId: string): UINode {
  return filterNode(spec, (node) => node.id !== nodeId);
}

function increaseMaxLines(
  spec: UINode,
  nodeId: string,
  lines: number,
): UINode {
  return mapNode(spec, (node) => {
    if (node.id !== nodeId || node.kind !== 'textBlock') return node;
    return { ...node, constraints: { ...node.constraints, maxLines: lines } };
  });
}

// Helper: check whether a UINode carries children
function hasChildren(
  node: UINode,
): node is UINode & { children: UINode[] } {
  return (
    'children' in node &&
    Array.isArray((node as Record<string, unknown>).children)
  );
}

// Helper: recursively map over UINode tree
function mapNode(node: UINode, fn: (node: UINode) => UINode): UINode {
  const mapped = fn(node);
  if (hasChildren(mapped)) {
    const newChildren = mapped.children.map((c) => mapNode(c, fn));
    // Use Object.assign to avoid discriminated-union spread issues
    return Object.assign({}, mapped, { children: newChildren }) as UINode;
  }
  return mapped;
}

// Helper: recursively filter UINode tree (remove nodes matching predicate)
function filterNode(node: UINode, keep: (node: UINode) => boolean): UINode {
  if (!keep(node)) {
    // Return a minimal placeholder if root is filtered
    return { kind: 'column', id: `${node.id}-removed`, children: [], gap: 0 };
  }
  if (hasChildren(node)) {
    const newChildren = node.children
      .filter(keep)
      .map((c) => filterNode(c, keep));
    return Object.assign({}, node, { children: newChildren }) as UINode;
  }
  return node;
}

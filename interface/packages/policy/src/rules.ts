import type {
  UINode,
  SolvedLayout,
  SolvedNode,
  Violation,
} from '@life/ikr-ir';

export type PolicyRule = {
  name: string;
  evaluate: (spec: UINode, solved: SolvedLayout) => Violation[];
};

/** Text exceeds maxLines constraint */
export const maxLinesRule: PolicyRule = {
  name: 'maxLines',
  evaluate: (spec, solved) => {
    const violations: Violation[] = [];
    visit(spec, (node) => {
      if (node.kind === 'textBlock' && node.constraints?.maxLines) {
        const solvedNode = solved.nodes.find((n) => n.id === node.id);
        if (
          solvedNode?.lineCount &&
          solvedNode.lineCount > node.constraints.maxLines
        ) {
          violations.push({
            nodeId: node.id,
            rule: 'maxLines',
            severity: 'error',
            actual: solvedNode.lineCount,
            limit: node.constraints.maxLines,
            repairOptions: [
              {
                kind: 'summarize_text',
                nodeId: node.id,
                targetChars: estimateCharsForLines(
                  solvedNode,
                  node.constraints.maxLines,
                ),
              },
              {
                kind: 'increase_max_lines',
                nodeId: node.id,
                lines: solvedNode.lineCount,
              },
            ],
          });
        }
      }
    });
    return violations;
  },
};

/** Node exceeds container bounds */
export const overflowRule: PolicyRule = {
  name: 'overflow',
  evaluate: (_spec, solved) => {
    const violations: Violation[] = [];
    for (const node of solved.nodes) {
      if (node.overflow) {
        violations.push({
          nodeId: node.id,
          rule: 'overflow',
          severity: 'error',
          actual: node.height,
          limit: node.height,
          repairOptions: [
            { kind: 'summarize_text', nodeId: node.id, targetChars: 100 },
            { kind: 'hide_node', nodeId: node.id },
          ],
        });
      }
    }
    return violations;
  },
};

/** Interactive element smaller than 44px touch target */
export const minTouchTargetRule: PolicyRule = {
  name: 'minTouchTarget',
  evaluate: (spec, solved) => {
    const violations: Violation[] = [];
    visit(spec, (node) => {
      if (node.kind === 'button') {
        const solvedNode = solved.nodes.find((n) => n.id === node.id);
        if (
          solvedNode &&
          (solvedNode.width < 44 || solvedNode.height < 44)
        ) {
          violations.push({
            nodeId: node.id,
            rule: 'minTouchTarget',
            severity: 'warning',
            actual: Math.min(solvedNode.width, solvedNode.height),
            limit: 44,
            repairOptions: [
              { kind: 'widen_container', nodeId: node.id, targetWidth: 44 },
            ],
          });
        }
      }
    });
    return violations;
  },
};

// Helper: walk all nodes in a UINode tree
function visit(node: UINode, fn: (node: UINode) => void): void {
  fn(node);
  if ('children' in node && Array.isArray((node as Record<string, unknown>).children)) {
    for (const child of (node as unknown as { children: UINode[] }).children) {
      visit(child, fn);
    }
  }
}

// Helper: estimate chars that would fit in N lines given current width
function estimateCharsForLines(
  node: SolvedNode,
  targetLines: number,
): number {
  if (!node.text || !node.lineCount || node.lineCount === 0) return 100;
  const charsPerLine = Math.ceil(node.text.length / node.lineCount);
  return charsPerLine * targetLines;
}

export const defaultRules: PolicyRule[] = [
  maxLinesRule,
  overflowRule,
  minTouchTargetRule,
];

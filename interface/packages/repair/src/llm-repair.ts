import type { UINode, Violation } from '@life/ikr-ir';

/** Repair patch from LLM */
export type RepairPatch = {
  nodeId: string;
  action: 'rewrite_text' | 'rewrite_label';
  newText: string;
};

/**
 * Build a prompt for the LLM to generate repair patches.
 */
export function buildRepairPrompt(
  spec: UINode,
  violations: Violation[],
): string {
  const violationDescriptions = violations
    .map(
      (v) =>
        `- Node "${v.nodeId}" violates rule "${v.rule}": actual=${v.actual}, limit=${v.limit}`,
    )
    .join('\n');

  return `You are a UI layout repair agent. The following UI spec has constraint violations:

${violationDescriptions}

For each violation, suggest a text rewrite that fixes it. Return a JSON array of patches:
[{ "nodeId": "...", "action": "rewrite_text", "newText": "..." }]

Rules:
- Preserve meaning while shortening text
- Do not add information that wasn't in the original
- Target the specific character count needed to fit constraints
- Return only the JSON array, no explanation`;
}

/**
 * Apply LLM-generated patches to a spec.
 */
export function applyRepairPatches(
  spec: UINode,
  patches: RepairPatch[],
): UINode {
  let current = structuredClone(spec);
  for (const patch of patches) {
    current = applyPatch(current, patch);
  }
  return current;
}

function applyPatch(node: UINode, patch: RepairPatch): UINode {
  if (node.id === patch.nodeId) {
    if (node.kind === 'textBlock' && patch.action === 'rewrite_text') {
      return { ...node, text: patch.newText };
    }
    if (node.kind === 'chip' && patch.action === 'rewrite_label') {
      return { ...node, label: patch.newText };
    }
    if (node.kind === 'button' && patch.action === 'rewrite_label') {
      return { ...node, label: patch.newText };
    }
  }
  if (
    'children' in node &&
    Array.isArray((node as Record<string, unknown>).children)
  ) {
    const parent = node as UINode & { children: UINode[] };
    const newChildren = parent.children.map((c: UINode) =>
      applyPatch(c, patch),
    );
    return Object.assign({}, parent, { children: newChildren }) as UINode;
  }
  return node;
}

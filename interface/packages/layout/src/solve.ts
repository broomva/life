/**
 * Main layout solver — the public API of @life/ikr-layout.
 *
 * Takes a UINode tree and LayoutConstraints, runs Yoga flexbox layout
 * with pluggable text measurement, and returns a SolvedLayout.
 */
import Yoga from 'yoga-layout';
import type { UINode, LayoutConstraints, SolvedLayout } from '@life/ikr-ir';
import {
  buildYogaTree,
  extractSolvedNodes,
  type TextMeasureFn,
} from './box-layout.js';
import { measureMonoText } from './text-mono.js';

/**
 * Solve layout for a UINode tree given a set of constraints.
 *
 * @param spec        — Root UINode of the tree to lay out.
 * @param constraints — Width, optional height, surface type.
 * @param customMeasure — Optional custom text measure function.
 *                        If not provided, uses monospace measurement for
 *                        terminal surfaces and a character-width estimate
 *                        for others.
 * @returns A SolvedLayout with positions for every node.
 */
export function solveLayout(
  spec: UINode,
  constraints: LayoutConstraints,
  customMeasure?: TextMeasureFn,
): SolvedLayout {
  const lineHeight =
    constraints.lineHeight ??
    (constraints.surface.kind === 'terminal' ? 1 : 20);

  const measureText: TextMeasureFn =
    customMeasure ??
    ((text, maxWidth) => {
      if (constraints.surface.kind === 'terminal') {
        return measureMonoText(text, maxWidth);
      }
      // Default character-width estimate for non-terminal surfaces
      // when no custom measure function is provided.
      const avgCharWidth = lineHeight * 0.5;
      const textWidth = text.length * avgCharWidth;
      if (textWidth <= maxWidth) {
        return { lineCount: 1, height: lineHeight, width: textWidth };
      }
      const lc = Math.ceil(textWidth / maxWidth);
      return { lineCount: lc, height: lc * lineHeight, width: maxWidth };
    });

  const { yogaNode, nodeMap } = buildYogaTree(spec, measureText, lineHeight);

  // Apply root constraints
  yogaNode.setWidth(constraints.width);
  if (constraints.height !== undefined) {
    yogaNode.setHeight(constraints.height);
  }

  // Run Yoga layout pass
  yogaNode.calculateLayout(
    constraints.width,
    constraints.height ?? undefined,
    Yoga.DIRECTION_LTR,
  );

  const nodes = extractSolvedNodes(nodeMap, measureText);
  const rootLayout = yogaNode.getComputedLayout();

  // Clean up Yoga nodes to prevent memory leaks
  yogaNode.freeRecursive();

  return {
    valid: true, // Violations are computed by ikr-policy, not here
    width: rootLayout.width,
    height: rootLayout.height,
    nodes,
    violations: [],
  };
}

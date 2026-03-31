/**
 * Yoga-backed box layout engine.
 *
 * Builds a Yoga node tree from a UINode, wires text measurement via
 * a pluggable TextMeasureFn, then extracts solved positions after
 * Yoga's layout pass.
 */
import Yoga, {
  type Node as YogaNode,
  type MeasureFunction,
} from 'yoga-layout';
import type { UINode, SolvedNode } from '@life/ikr-ir';

/** Pluggable text measurement function. */
export type TextMeasureFn = (
  text: string,
  maxWidth: number,
) => { lineCount: number; height: number; width: number };

type NodeEntry = { uiNode: UINode; yogaNode: YogaNode };

/**
 * Build a Yoga flexbox tree from a UINode tree.
 *
 * Returns the root Yoga node and a map from UINode.id → { uiNode, yogaNode }
 * so that solved positions can be extracted after calculateLayout().
 */
export function buildYogaTree(
  node: UINode,
  measureText: TextMeasureFn,
  lineHeight: number,
): { yogaNode: YogaNode; nodeMap: Map<string, NodeEntry> } {
  const nodeMap = new Map<string, NodeEntry>();

  function build(uiNode: UINode): YogaNode {
    const yn = Yoga.Node.create();
    nodeMap.set(uiNode.id, { uiNode, yogaNode: yn });

    switch (uiNode.kind) {
      case 'textBlock': {
        // Text nodes are leaf nodes with a measure function
        const text = uiNode.text;
        const measureFn: MeasureFunction = (width, widthMode) => {
          const maxW =
            widthMode === Yoga.MEASURE_MODE_UNDEFINED ? Infinity : width;
          const result = measureText(text, maxW);
          return {
            width: result.width,
            height: result.height * lineHeight,
          };
        };
        yn.setMeasureFunc(measureFn);
        break;
      }

      case 'card': {
        yn.setFlexDirection(Yoga.FLEX_DIRECTION_COLUMN);
        yn.setPadding(Yoga.EDGE_ALL, uiNode.padding);
        for (let i = 0; i < uiNode.children.length; i++) {
          yn.insertChild(build(uiNode.children[i]), i);
        }
        break;
      }

      case 'column': {
        yn.setFlexDirection(Yoga.FLEX_DIRECTION_COLUMN);
        yn.setGap(Yoga.GUTTER_ROW, uiNode.gap);
        for (let i = 0; i < uiNode.children.length; i++) {
          yn.insertChild(build(uiNode.children[i]), i);
        }
        break;
      }

      case 'inlineRow': {
        yn.setFlexDirection(Yoga.FLEX_DIRECTION_ROW);
        yn.setFlexWrap(
          uiNode.wrap ? Yoga.WRAP_WRAP : Yoga.WRAP_NO_WRAP,
        );
        yn.setGap(Yoga.GUTTER_COLUMN, uiNode.gap);
        for (let i = 0; i < uiNode.children.length; i++) {
          yn.insertChild(build(uiNode.children[i]), i);
        }
        break;
      }

      case 'chip': {
        // Chips are measured as inline text with padding
        const label = uiNode.label;
        const chipMeasure: MeasureFunction = (width) => {
          const result = measureText(label, width);
          return {
            width: result.width + 16, // horizontal padding
            height: result.height * lineHeight + 8, // vertical padding
          };
        };
        yn.setMeasureFunc(chipMeasure);
        break;
      }

      case 'button': {
        const btnLabel = uiNode.label;
        const btnMeasure: MeasureFunction = (width) => {
          const result = measureText(btnLabel, width);
          return {
            width: Math.max(result.width + 24, 80), // min 80px button width
            height: result.height * lineHeight + 16,
          };
        };
        yn.setMeasureFunc(btnMeasure);
        break;
      }

      case 'icon': {
        yn.setWidth(uiNode.size);
        yn.setHeight(uiNode.size);
        break;
      }

      case 'section': {
        yn.setFlexDirection(Yoga.FLEX_DIRECTION_COLUMN);
        yn.setGap(Yoga.GUTTER_ROW, 8);

        // Title is an implicit first child measured as text
        const titleText = uiNode.title;
        const titleNode = Yoga.Node.create();
        const titleMeasure: MeasureFunction = (width) => {
          const result = measureText(titleText, width);
          return {
            width: result.width,
            height: result.height * lineHeight,
          };
        };
        titleNode.setMeasureFunc(titleMeasure);
        yn.insertChild(titleNode, 0);

        for (let i = 0; i < uiNode.children.length; i++) {
          yn.insertChild(build(uiNode.children[i]), i + 1);
        }
        break;
      }
    }

    return yn;
  }

  const root = build(node);
  return { yogaNode: root, nodeMap };
}

/**
 * Walk the nodeMap after calculateLayout() and extract solved positions.
 */
export function extractSolvedNodes(
  nodeMap: Map<string, NodeEntry>,
  measureText: TextMeasureFn,
): SolvedNode[] {
  const result: SolvedNode[] = [];

  for (const [id, { uiNode, yogaNode }] of nodeMap) {
    const layout = yogaNode.getComputedLayout();
    const solvedNode: SolvedNode = {
      id,
      x: layout.left,
      y: layout.top,
      width: layout.width,
      height: layout.height,
      overflow: false,
    };

    // Calculate line count for text nodes and check overflow
    if (uiNode.kind === 'textBlock') {
      const measured = measureText(uiNode.text, layout.width);
      solvedNode.lineCount = measured.lineCount;
      solvedNode.text = uiNode.text;
      if (
        uiNode.constraints?.maxLines &&
        measured.lineCount > uiNode.constraints.maxLines
      ) {
        solvedNode.overflow = true;
      }
    }

    result.push(solvedNode);
  }

  return result;
}

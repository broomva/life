import { describe, expect, it } from 'vitest';
import type {
  UINode,
  SolvedLayout,
  TextBlockNode,
  ButtonNode,
  CardNode,
} from '@life/ikr-ir';
import {
  maxLinesRule,
  overflowRule,
  minTouchTargetRule,
} from '../rules.js';

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function makeSolvedLayout(
  nodes: SolvedLayout['nodes'],
  overrides?: Partial<SolvedLayout>,
): SolvedLayout {
  return {
    valid: true,
    width: 400,
    height: 800,
    nodes,
    violations: [],
    ...overrides,
  };
}

// ---------------------------------------------------------------------------
// maxLinesRule
// ---------------------------------------------------------------------------

describe('maxLinesRule', () => {
  it('detects when lineCount exceeds maxLines', () => {
    const spec: TextBlockNode = {
      kind: 'textBlock',
      id: 'txt-1',
      text: 'A long paragraph that wraps across many lines in the layout engine.',
      role: 'body',
      fontToken: 'body-md',
      constraints: { maxLines: 2 },
    };

    const solved = makeSolvedLayout([
      {
        id: 'txt-1',
        x: 0,
        y: 0,
        width: 200,
        height: 60,
        lineCount: 5,
        overflow: false,
        text: spec.text,
      },
    ]);

    const violations = maxLinesRule.evaluate(spec, solved);

    expect(violations).toHaveLength(1);
    expect(violations[0].nodeId).toBe('txt-1');
    expect(violations[0].rule).toBe('maxLines');
    expect(violations[0].severity).toBe('error');
    expect(violations[0].actual).toBe(5);
    expect(violations[0].limit).toBe(2);

    // Should include summarize_text and increase_max_lines repair options
    expect(violations[0].repairOptions).toHaveLength(2);
    expect(violations[0].repairOptions[0].kind).toBe('summarize_text');
    expect(violations[0].repairOptions[1].kind).toBe('increase_max_lines');
  });

  it('passes when lineCount is within maxLines', () => {
    const spec: TextBlockNode = {
      kind: 'textBlock',
      id: 'txt-2',
      text: 'Short text.',
      role: 'body',
      fontToken: 'body-md',
      constraints: { maxLines: 3 },
    };

    const solved = makeSolvedLayout([
      {
        id: 'txt-2',
        x: 0,
        y: 0,
        width: 200,
        height: 40,
        lineCount: 2,
        overflow: false,
        text: spec.text,
      },
    ]);

    const violations = maxLinesRule.evaluate(spec, solved);
    expect(violations).toHaveLength(0);
  });

  it('passes when textBlock has no maxLines constraint', () => {
    const spec: TextBlockNode = {
      kind: 'textBlock',
      id: 'txt-3',
      text: 'Unconstrained text.',
      role: 'body',
      fontToken: 'body-md',
    };

    const solved = makeSolvedLayout([
      {
        id: 'txt-3',
        x: 0,
        y: 0,
        width: 200,
        height: 100,
        lineCount: 10,
        overflow: false,
        text: spec.text,
      },
    ]);

    const violations = maxLinesRule.evaluate(spec, solved);
    expect(violations).toHaveLength(0);
  });

  it('walks nested children to find textBlock nodes', () => {
    const textNode: TextBlockNode = {
      kind: 'textBlock',
      id: 'nested-txt',
      text: 'Deeply nested text that overflows.',
      role: 'body',
      fontToken: 'body-md',
      constraints: { maxLines: 1 },
    };

    const card: CardNode = {
      kind: 'card',
      id: 'card-1',
      children: [textNode],
      padding: 16,
      widthPolicy: 'fill',
    };

    const solved = makeSolvedLayout([
      { id: 'card-1', x: 0, y: 0, width: 400, height: 200, overflow: false },
      {
        id: 'nested-txt',
        x: 16,
        y: 16,
        width: 368,
        height: 60,
        lineCount: 3,
        overflow: false,
        text: textNode.text,
      },
    ]);

    const violations = maxLinesRule.evaluate(card, solved);
    expect(violations).toHaveLength(1);
    expect(violations[0].nodeId).toBe('nested-txt');
  });
});

// ---------------------------------------------------------------------------
// overflowRule
// ---------------------------------------------------------------------------

describe('overflowRule', () => {
  it('detects overflow nodes', () => {
    const spec: TextBlockNode = {
      kind: 'textBlock',
      id: 'txt-overflow',
      text: 'Some text',
      role: 'body',
      fontToken: 'body-md',
    };

    const solved = makeSolvedLayout([
      {
        id: 'txt-overflow',
        x: 0,
        y: 0,
        width: 200,
        height: 300,
        overflow: true,
        text: 'Some text',
      },
    ]);

    const violations = overflowRule.evaluate(spec, solved);

    expect(violations).toHaveLength(1);
    expect(violations[0].nodeId).toBe('txt-overflow');
    expect(violations[0].rule).toBe('overflow');
    expect(violations[0].severity).toBe('error');
    expect(violations[0].repairOptions).toHaveLength(2);
    expect(violations[0].repairOptions[0].kind).toBe('summarize_text');
    expect(violations[0].repairOptions[1].kind).toBe('hide_node');
  });

  it('passes when no nodes overflow', () => {
    const spec: TextBlockNode = {
      kind: 'textBlock',
      id: 'txt-ok',
      text: 'Fine text',
      role: 'body',
      fontToken: 'body-md',
    };

    const solved = makeSolvedLayout([
      {
        id: 'txt-ok',
        x: 0,
        y: 0,
        width: 200,
        height: 40,
        overflow: false,
      },
    ]);

    const violations = overflowRule.evaluate(spec, solved);
    expect(violations).toHaveLength(0);
  });
});

// ---------------------------------------------------------------------------
// minTouchTargetRule
// ---------------------------------------------------------------------------

describe('minTouchTargetRule', () => {
  it('detects buttons smaller than 44px', () => {
    const spec: ButtonNode = {
      kind: 'button',
      id: 'btn-small',
      label: 'X',
      action: 'close',
    };

    const solved = makeSolvedLayout([
      {
        id: 'btn-small',
        x: 0,
        y: 0,
        width: 30,
        height: 30,
        overflow: false,
      },
    ]);

    const violations = minTouchTargetRule.evaluate(spec, solved);

    expect(violations).toHaveLength(1);
    expect(violations[0].nodeId).toBe('btn-small');
    expect(violations[0].rule).toBe('minTouchTarget');
    expect(violations[0].severity).toBe('warning');
    expect(violations[0].actual).toBe(30);
    expect(violations[0].limit).toBe(44);
    expect(violations[0].repairOptions).toHaveLength(1);
    expect(violations[0].repairOptions[0].kind).toBe('widen_container');
  });

  it('detects when only height is below 44px', () => {
    const spec: ButtonNode = {
      kind: 'button',
      id: 'btn-flat',
      label: 'Submit',
      action: 'submit',
    };

    const solved = makeSolvedLayout([
      {
        id: 'btn-flat',
        x: 0,
        y: 0,
        width: 120,
        height: 32,
        overflow: false,
      },
    ]);

    const violations = minTouchTargetRule.evaluate(spec, solved);
    expect(violations).toHaveLength(1);
    expect(violations[0].actual).toBe(32);
  });

  it('passes when button meets 44px threshold', () => {
    const spec: ButtonNode = {
      kind: 'button',
      id: 'btn-ok',
      label: 'OK',
      action: 'confirm',
    };

    const solved = makeSolvedLayout([
      {
        id: 'btn-ok',
        x: 0,
        y: 0,
        width: 80,
        height: 48,
        overflow: false,
      },
    ]);

    const violations = minTouchTargetRule.evaluate(spec, solved);
    expect(violations).toHaveLength(0);
  });

  it('ignores non-button nodes', () => {
    const spec: TextBlockNode = {
      kind: 'textBlock',
      id: 'txt-small',
      text: 'Tiny',
      role: 'caption',
      fontToken: 'caption-sm',
    };

    const solved = makeSolvedLayout([
      {
        id: 'txt-small',
        x: 0,
        y: 0,
        width: 20,
        height: 12,
        overflow: false,
      },
    ]);

    const violations = minTouchTargetRule.evaluate(spec, solved);
    expect(violations).toHaveLength(0);
  });
});

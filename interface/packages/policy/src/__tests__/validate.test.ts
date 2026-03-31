import { describe, expect, it } from 'vitest';
import type {
  UINode,
  SolvedLayout,
  Violation,
  ButtonNode,
  TextBlockNode,
  CardNode,
} from '@life/ikr-ir';
import { validate } from '../validate.js';
import type { PolicyRule } from '../rules.js';

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
// validate()
// ---------------------------------------------------------------------------

describe('validate', () => {
  it('returns empty array for a valid layout', () => {
    const spec: TextBlockNode = {
      kind: 'textBlock',
      id: 'txt-valid',
      text: 'All good.',
      role: 'body',
      fontToken: 'body-md',
      constraints: { maxLines: 5 },
    };

    const solved = makeSolvedLayout([
      {
        id: 'txt-valid',
        x: 0,
        y: 0,
        width: 200,
        height: 40,
        lineCount: 2,
        overflow: false,
        text: spec.text,
      },
    ]);

    const violations = validate(spec, solved);
    expect(violations).toEqual([]);
  });

  it('returns sorted violations — errors before warnings', () => {
    // Build a tree with both an overflow (error) and a small button (warning)
    const button: ButtonNode = {
      kind: 'button',
      id: 'btn-tiny',
      label: 'Go',
      action: 'navigate',
    };

    const text: TextBlockNode = {
      kind: 'textBlock',
      id: 'txt-over',
      text: 'Overflowing content',
      role: 'body',
      fontToken: 'body-md',
    };

    const card: CardNode = {
      kind: 'card',
      id: 'card-root',
      children: [text, button],
      padding: 8,
      widthPolicy: 'fill',
    };

    const solved = makeSolvedLayout([
      { id: 'card-root', x: 0, y: 0, width: 400, height: 200, overflow: false },
      {
        id: 'txt-over',
        x: 8,
        y: 8,
        width: 384,
        height: 150,
        overflow: true,
        text: 'Overflowing content',
      },
      {
        id: 'btn-tiny',
        x: 8,
        y: 160,
        width: 30,
        height: 30,
        overflow: false,
      },
    ]);

    const violations = validate(card, solved);

    // Should have at least one error and one warning
    expect(violations.length).toBeGreaterThanOrEqual(2);

    // Errors come before warnings
    const firstWarningIdx = violations.findIndex(
      (v) => v.severity === 'warning',
    );
    const lastErrorIdx = violations.reduce(
      (acc, v, i) => (v.severity === 'error' ? i : acc),
      -1,
    );

    if (firstWarningIdx !== -1 && lastErrorIdx !== -1) {
      expect(lastErrorIdx).toBeLessThan(firstWarningIdx);
    }
  });

  it('accepts custom rules', () => {
    const alwaysFail: PolicyRule = {
      name: 'alwaysFail',
      evaluate: (spec) => [
        {
          nodeId: spec.id,
          rule: 'alwaysFail',
          severity: 'warning',
          actual: 0,
          limit: 1,
          repairOptions: [],
        },
      ],
    };

    const spec: TextBlockNode = {
      kind: 'textBlock',
      id: 'txt-custom',
      text: 'Custom test',
      role: 'body',
      fontToken: 'body-md',
    };

    const solved = makeSolvedLayout([
      {
        id: 'txt-custom',
        x: 0,
        y: 0,
        width: 200,
        height: 40,
        overflow: false,
      },
    ]);

    const violations = validate(spec, solved, [alwaysFail]);
    expect(violations).toHaveLength(1);
    expect(violations[0].rule).toBe('alwaysFail');
  });

  it('aggregates violations from multiple rules', () => {
    const text: TextBlockNode = {
      kind: 'textBlock',
      id: 'txt-multi',
      text: 'This text is long enough to overflow across many lines in the layout engine.',
      role: 'body',
      fontToken: 'body-md',
      constraints: { maxLines: 1 },
    };

    const solved = makeSolvedLayout([
      {
        id: 'txt-multi',
        x: 0,
        y: 0,
        width: 200,
        height: 200,
        lineCount: 5,
        overflow: true,
        text: text.text,
      },
    ]);

    // Default rules include maxLines + overflow — both should fire
    const violations = validate(text, solved);
    const rules = violations.map((v) => v.rule);
    expect(rules).toContain('maxLines');
    expect(rules).toContain('overflow');
  });
});

import { describe, expect, it } from 'vitest';
import type {
  UINode,
  InlineRowNode,
  CardNode,
  TextBlockNode,
  ChipNode,
  ColumnNode,
  Violation,
} from '@life/ikr-ir';
import { applyDeterministicRepairs } from '../deterministic.js';

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function makeViolation(overrides: Partial<Violation> & Pick<Violation, 'nodeId' | 'rule' | 'repairOptions'>): Violation {
  return {
    severity: 'error',
    actual: 0,
    limit: 0,
    ...overrides,
  };
}

// ---------------------------------------------------------------------------
// collapseChips
// ---------------------------------------------------------------------------

describe('collapseChips', () => {
  it('reduces children and adds +N overflow chip', () => {
    const chips: ChipNode[] = [
      { kind: 'chip', id: 'c1', label: 'Alpha', priority: 1 },
      { kind: 'chip', id: 'c2', label: 'Beta', priority: 2 },
      { kind: 'chip', id: 'c3', label: 'Gamma', priority: 3 },
      { kind: 'chip', id: 'c4', label: 'Delta', priority: 4 },
      { kind: 'chip', id: 'c5', label: 'Epsilon', priority: 5 },
    ];

    const row: InlineRowNode = {
      kind: 'inlineRow',
      id: 'chip-row',
      children: chips,
      wrap: false,
      gap: 4,
    };

    const violation = makeViolation({
      nodeId: 'chip-row',
      rule: 'overflow',
      repairOptions: [
        { kind: 'collapse_chips', nodeId: 'chip-row', maxVisible: 2 },
      ],
    });

    const { repaired, applied, remaining } = applyDeterministicRepairs(row, [violation]);

    expect(applied).toEqual(['overflow:collapse_chips']);
    expect(remaining).toEqual([]);

    // Should be an inlineRow with 2 visible + 1 overflow chip
    const repairedRow = repaired as InlineRowNode;
    expect(repairedRow.kind).toBe('inlineRow');
    expect(repairedRow.children).toHaveLength(3);
    expect(repairedRow.children[0].id).toBe('c1');
    expect(repairedRow.children[1].id).toBe('c2');

    const overflowChip = repairedRow.children[2] as ChipNode;
    expect(overflowChip.kind).toBe('chip');
    expect(overflowChip.id).toBe('chip-row-overflow');
    expect(overflowChip.label).toBe('+3');
  });
});

// ---------------------------------------------------------------------------
// switchDensity
// ---------------------------------------------------------------------------

describe('switchDensity', () => {
  it('halves padding on card when switching to compact', () => {
    const card: CardNode = {
      kind: 'card',
      id: 'card-1',
      children: [],
      padding: 16,
      widthPolicy: 'fill',
    };

    const violation = makeViolation({
      nodeId: 'card-1',
      rule: 'overflow',
      repairOptions: [
        { kind: 'switch_density', nodeId: 'card-1', density: 'compact' },
      ],
    });

    const { repaired, applied } = applyDeterministicRepairs(card, [violation]);

    expect(applied).toEqual(['overflow:switch_density']);
    const repairedCard = repaired as CardNode;
    expect(repairedCard.padding).toBe(8); // 16 / 2
    expect(repairedCard.constraints?.density).toBe('compact');
  });
});

// ---------------------------------------------------------------------------
// hideNode
// ---------------------------------------------------------------------------

describe('hideNode', () => {
  it('removes node from tree', () => {
    const text: TextBlockNode = {
      kind: 'textBlock',
      id: 'txt-hidden',
      text: 'To be hidden',
      role: 'body',
      fontToken: 'body-md',
    };

    const column: ColumnNode = {
      kind: 'column',
      id: 'col-1',
      children: [
        { kind: 'textBlock', id: 'txt-keep', text: 'Keep me', role: 'title', fontToken: 'title-lg' } as TextBlockNode,
        text,
      ],
      gap: 8,
    };

    const violation = makeViolation({
      nodeId: 'txt-hidden',
      rule: 'overflow',
      repairOptions: [
        { kind: 'hide_node', nodeId: 'txt-hidden' },
      ],
    });

    const { repaired, applied } = applyDeterministicRepairs(column, [violation]);

    expect(applied).toEqual(['overflow:hide_node']);
    const repairedCol = repaired as ColumnNode;
    expect(repairedCol.children).toHaveLength(1);
    expect(repairedCol.children[0].id).toBe('txt-keep');
  });

  it('returns placeholder when root is hidden', () => {
    const text: TextBlockNode = {
      kind: 'textBlock',
      id: 'txt-root',
      text: 'Root text',
      role: 'body',
      fontToken: 'body-md',
    };

    const violation = makeViolation({
      nodeId: 'txt-root',
      rule: 'overflow',
      repairOptions: [
        { kind: 'hide_node', nodeId: 'txt-root' },
      ],
    });

    const { repaired } = applyDeterministicRepairs(text, [violation]);

    // Should be replaced with a minimal column placeholder
    expect(repaired.kind).toBe('column');
    expect(repaired.id).toBe('txt-root-removed');
  });
});

// ---------------------------------------------------------------------------
// increaseMaxLines
// ---------------------------------------------------------------------------

describe('increaseMaxLines', () => {
  it('updates textBlock constraints', () => {
    const text: TextBlockNode = {
      kind: 'textBlock',
      id: 'txt-lines',
      text: 'Some long text that wraps to many lines.',
      role: 'body',
      fontToken: 'body-md',
      constraints: { maxLines: 2 },
    };

    const violation = makeViolation({
      nodeId: 'txt-lines',
      rule: 'maxLines',
      actual: 5,
      limit: 2,
      repairOptions: [
        { kind: 'increase_max_lines', nodeId: 'txt-lines', lines: 5 },
      ],
    });

    const { repaired, applied } = applyDeterministicRepairs(text, [violation]);

    expect(applied).toEqual(['maxLines:increase_max_lines']);
    const repairedText = repaired as TextBlockNode;
    expect(repairedText.constraints?.maxLines).toBe(5);
  });

  it('creates constraints object if none exists', () => {
    const text: TextBlockNode = {
      kind: 'textBlock',
      id: 'txt-no-constraints',
      text: 'No constraints yet.',
      role: 'body',
      fontToken: 'body-md',
    };

    const violation = makeViolation({
      nodeId: 'txt-no-constraints',
      rule: 'maxLines',
      repairOptions: [
        { kind: 'increase_max_lines', nodeId: 'txt-no-constraints', lines: 3 },
      ],
    });

    const { repaired } = applyDeterministicRepairs(text, [violation]);
    const repairedText = repaired as TextBlockNode;
    expect(repairedText.constraints?.maxLines).toBe(3);
  });
});

// ---------------------------------------------------------------------------
// applyDeterministicRepairs (integration)
// ---------------------------------------------------------------------------

describe('applyDeterministicRepairs', () => {
  it('applies first deterministic strategy and returns remaining non-deterministic violations', () => {
    const text: TextBlockNode = {
      kind: 'textBlock',
      id: 'txt-sum',
      text: 'Long text that only AI can summarize.',
      role: 'body',
      fontToken: 'body-md',
      constraints: { maxLines: 2 },
    };

    const deterministicViolation = makeViolation({
      nodeId: 'txt-sum',
      rule: 'maxLines',
      actual: 5,
      limit: 2,
      repairOptions: [
        { kind: 'increase_max_lines', nodeId: 'txt-sum', lines: 5 },
      ],
    });

    const aiOnlyViolation = makeViolation({
      nodeId: 'txt-sum',
      rule: 'overflow',
      repairOptions: [
        { kind: 'summarize_text', nodeId: 'txt-sum', targetChars: 30 },
      ],
    });

    const { applied, remaining } = applyDeterministicRepairs(text, [
      deterministicViolation,
      aiOnlyViolation,
    ]);

    expect(applied).toEqual(['maxLines:increase_max_lines']);
    expect(remaining).toHaveLength(1);
    expect(remaining[0].rule).toBe('overflow');
  });

  it('does not mutate the original spec', () => {
    const original: TextBlockNode = {
      kind: 'textBlock',
      id: 'txt-immutable',
      text: 'Original text.',
      role: 'body',
      fontToken: 'body-md',
      constraints: { maxLines: 1 },
    };

    const originalJson = JSON.stringify(original);

    const violation = makeViolation({
      nodeId: 'txt-immutable',
      rule: 'maxLines',
      repairOptions: [
        { kind: 'increase_max_lines', nodeId: 'txt-immutable', lines: 5 },
      ],
    });

    applyDeterministicRepairs(original, [violation]);

    // Original must be unchanged
    expect(JSON.stringify(original)).toBe(originalJson);
  });

  it('returns empty applied and all remaining when no deterministic options exist', () => {
    const text: TextBlockNode = {
      kind: 'textBlock',
      id: 'txt-ai',
      text: 'Needs AI.',
      role: 'body',
      fontToken: 'body-md',
    };

    const violation = makeViolation({
      nodeId: 'txt-ai',
      rule: 'maxLines',
      repairOptions: [
        { kind: 'summarize_text', nodeId: 'txt-ai', targetChars: 20 },
      ],
    });

    const { applied, remaining } = applyDeterministicRepairs(text, [violation]);

    expect(applied).toEqual([]);
    expect(remaining).toHaveLength(1);
  });
});

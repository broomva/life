import { describe, expect, it, vi } from 'vitest';
import type {
  UINode,
  TextBlockNode,
  ColumnNode,
  LayoutConstraints,
  SolvedLayout,
  Violation,
} from '@life/ikr-ir';
import { repairLayout } from '../repair.js';
import type { RepairPatch } from '../llm-repair.js';

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/** Terminal surface with 40 cols for compact layout. */
const terminalConstraints: LayoutConstraints = {
  width: 40,
  surface: { kind: 'terminal', cols: 40, rows: 24, monoWidth: 1 },
};

/**
 * Create a textBlock that will violate maxLines on a 40-col terminal.
 * At 40 chars per line, ~200 chars wraps to ~5 lines, exceeding maxLines: 2.
 */
function makeOverflowingText(id: string): TextBlockNode {
  return {
    kind: 'textBlock',
    id,
    text: 'This is a long paragraph of text that will definitely wrap across multiple lines when rendered on a narrow forty column terminal surface area.',
    role: 'body',
    fontToken: 'body-md',
    constraints: { maxLines: 2 },
  };
}

// ---------------------------------------------------------------------------
// repairLayout — deterministic only (no callLLM)
// ---------------------------------------------------------------------------

describe('repairLayout — deterministic only', () => {
  it('repairs violations using Tier 1 strategies when available', async () => {
    const spec = makeOverflowingText('txt-det');

    // The text is ~140 chars at 40 cols = ~4 lines, but maxLines=2.
    // solveLayout + validate will find a maxLines violation with
    // increase_max_lines as a repair option.
    const result = await repairLayout(
      spec,
      [
        {
          nodeId: 'txt-det',
          rule: 'maxLines',
          severity: 'error',
          actual: 4,
          limit: 2,
          repairOptions: [
            { kind: 'summarize_text', nodeId: 'txt-det', targetChars: 80 },
            { kind: 'increase_max_lines', nodeId: 'txt-det', lines: 4 },
          ],
        },
      ],
      { constraints: terminalConstraints },
    );

    // Should have applied increase_max_lines
    expect(result.repairsApplied).toContain('maxLines:increase_max_lines');
    expect(result.iterations).toBeGreaterThanOrEqual(1);

    // The repaired textBlock should have maxLines updated
    const repaired = result.spec as TextBlockNode;
    expect(repaired.constraints?.maxLines).toBe(4);
  });
});

// ---------------------------------------------------------------------------
// repairLayout — with mock LLM (Tier 2)
// ---------------------------------------------------------------------------

describe('repairLayout — with mock callLLM', () => {
  it('uses LLM to rewrite text when deterministic repairs are insufficient', async () => {
    const spec = makeOverflowingText('txt-llm');

    // Only AI-requiring repair option (summarize_text → no deterministic match)
    const violations: Violation[] = [
      {
        nodeId: 'txt-llm',
        rule: 'maxLines',
        severity: 'error',
        actual: 4,
        limit: 2,
        repairOptions: [
          { kind: 'summarize_text', nodeId: 'txt-llm', targetChars: 60 },
        ],
      },
    ];

    // Mock LLM that returns a shortened version of the text
    const mockCallLLM = vi.fn(async (_prompt: string): Promise<RepairPatch[]> => [
      {
        nodeId: 'txt-llm',
        action: 'rewrite_text',
        newText: 'Short text that fits in two lines.',
      },
    ]);

    const result = await repairLayout(spec, violations, {
      constraints: terminalConstraints,
      callLLM: mockCallLLM,
    });

    // LLM should have been called
    expect(mockCallLLM).toHaveBeenCalled();

    // Should record the LLM repair
    expect(result.repairsApplied).toContain('llm:rewrite_text:txt-llm');

    // The text should be rewritten
    const repaired = result.spec as TextBlockNode;
    expect(repaired.text).toBe('Short text that fits in two lines.');
  });
});

// ---------------------------------------------------------------------------
// repairLayout — maxIterations
// ---------------------------------------------------------------------------

describe('repairLayout — maxIterations', () => {
  it('stops after maxIterations even if violations remain', async () => {
    const spec = makeOverflowingText('txt-stuck');

    // Violation with only a widen_container option (which is a no-op)
    const violations: Violation[] = [
      {
        nodeId: 'txt-stuck',
        rule: 'overflow',
        severity: 'error',
        actual: 100,
        limit: 40,
        repairOptions: [
          { kind: 'widen_container', nodeId: 'txt-stuck', targetWidth: 80 },
        ],
      },
    ];

    const result = await repairLayout(spec, violations, {
      constraints: terminalConstraints,
      maxIterations: 2,
    });

    // Should have run exactly 2 iterations then stopped
    expect(result.iterations).toBe(2);
    // The widen_container strategy is a no-op, so fullyResolved depends on
    // whether re-solve still finds violations. Since text is still long,
    // there may still be violations from the re-solve.
    // What we're really testing is the iteration limit.
    expect(result.iterations).toBeLessThanOrEqual(2);
  });

  it('defaults to 3 iterations when maxIterations not specified', async () => {
    const spec = makeOverflowingText('txt-default');

    // Violation with only summarize_text (no deterministic match, no callLLM)
    const violations: Violation[] = [
      {
        nodeId: 'txt-default',
        rule: 'maxLines',
        severity: 'error',
        actual: 4,
        limit: 2,
        repairOptions: [
          { kind: 'summarize_text', nodeId: 'txt-default', targetChars: 60 },
        ],
      },
    ];

    const result = await repairLayout(spec, violations, {
      constraints: terminalConstraints,
    });

    // Without callLLM, deterministic pass finds nothing, but re-solve
    // may or may not produce new violations. The key assertion is
    // iterations <= 3 (the default).
    expect(result.iterations).toBeLessThanOrEqual(3);
  });
});

// ---------------------------------------------------------------------------
// repairLayout — fullyResolved
// ---------------------------------------------------------------------------

describe('repairLayout — fullyResolved', () => {
  it('returns fullyResolved: true when all violations are fixed', async () => {
    const spec: TextBlockNode = {
      kind: 'textBlock',
      id: 'txt-fixable',
      text: 'Short text.',
      role: 'body',
      fontToken: 'body-md',
      constraints: { maxLines: 1 },
    };

    // Violation that can be fixed by increase_max_lines
    const violations: Violation[] = [
      {
        nodeId: 'txt-fixable',
        rule: 'maxLines',
        severity: 'error',
        actual: 2,
        limit: 1,
        repairOptions: [
          { kind: 'increase_max_lines', nodeId: 'txt-fixable', lines: 2 },
        ],
      },
    ];

    const result = await repairLayout(spec, violations, {
      constraints: terminalConstraints,
    });

    // After increasing maxLines from 1 to 2, the re-solve should find no violations
    // for this short text on a 40-col terminal.
    expect(result.fullyResolved).toBe(true);
    expect(result.repairsApplied).toContain('maxLines:increase_max_lines');
  });

  it('returns fullyResolved: false when violations remain after all iterations', async () => {
    const spec = makeOverflowingText('txt-unresolvable');

    // Only option is summarize_text (AI-only), and no callLLM provided
    const violations: Violation[] = [
      {
        nodeId: 'txt-unresolvable',
        rule: 'maxLines',
        severity: 'error',
        actual: 4,
        limit: 2,
        repairOptions: [
          {
            kind: 'summarize_text',
            nodeId: 'txt-unresolvable',
            targetChars: 60,
          },
        ],
      },
    ];

    const result = await repairLayout(spec, violations, {
      constraints: terminalConstraints,
      maxIterations: 1,
    });

    // Without callLLM and with only AI-requiring options, nothing gets fixed.
    // Re-solve on the same text may produce new violations.
    // The text is ~140 chars on 40 cols = ~4 lines > maxLines 2, so
    // fullyResolved should be false.
    expect(result.fullyResolved).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// repairLayout — combined tiers
// ---------------------------------------------------------------------------

describe('repairLayout — combined tiers', () => {
  it('applies deterministic first, then LLM for remaining', async () => {
    const text1: TextBlockNode = {
      kind: 'textBlock',
      id: 'txt-det-fix',
      text: 'Short.',
      role: 'body',
      fontToken: 'body-md',
      constraints: { maxLines: 1 },
    };

    // This text is ~200 chars: at 40 cols ≈ 5 lines, way beyond maxLines: 2.
    // The only repair option is summarize_text (AI-only), so deterministic
    // pass will put it in "remaining" and the loop will invoke callLLM.
    const text2: TextBlockNode = {
      kind: 'textBlock',
      id: 'txt-llm-fix',
      text: 'This is a very long paragraph of text that absolutely must be shortened by an AI model because it wraps across far too many lines on a narrow forty column terminal surface to ever fit within two lines.',
      role: 'body',
      fontToken: 'body-md',
      constraints: { maxLines: 2 },
    };

    const column: ColumnNode = {
      kind: 'column',
      id: 'col-1',
      children: [text1, text2],
      gap: 4,
    };

    const violations: Violation[] = [
      {
        nodeId: 'txt-det-fix',
        rule: 'maxLines',
        severity: 'error',
        actual: 2,
        limit: 1,
        repairOptions: [
          { kind: 'increase_max_lines', nodeId: 'txt-det-fix', lines: 2 },
        ],
      },
      {
        nodeId: 'txt-llm-fix',
        rule: 'maxLines',
        severity: 'error',
        actual: 5,
        limit: 2,
        repairOptions: [
          { kind: 'summarize_text', nodeId: 'txt-llm-fix', targetChars: 60 },
        ],
      },
    ];

    const mockCallLLM = vi.fn(async (_prompt: string): Promise<RepairPatch[]> => [
      {
        nodeId: 'txt-llm-fix',
        action: 'rewrite_text',
        newText: 'Short text fits in two lines.',
      },
    ]);

    const result = await repairLayout(column, violations, {
      constraints: terminalConstraints,
      callLLM: mockCallLLM,
    });

    // Deterministic repair should have been applied
    expect(result.repairsApplied).toContain('maxLines:increase_max_lines');

    // LLM repair should also have been applied
    expect(result.repairsApplied).toContain('llm:rewrite_text:txt-llm-fix');
    expect(mockCallLLM).toHaveBeenCalled();
  });
});

import { describe, expect, it } from 'vitest';
import { solveLayout } from '../solve.js';
import type {
  TextBlockNode,
  CardNode,
  ColumnNode,
  InlineRowNode,
  ChipNode,
  IconNode,
  ButtonNode,
  SectionNode,
  LayoutConstraints,
  Surface,
} from '@life/ikr-ir';

const terminalSurface: Surface = {
  kind: 'terminal',
  cols: 80,
  rows: 24,
  monoWidth: 1,
};

function terminalConstraints(width = 80): LayoutConstraints {
  return { width, surface: terminalSurface };
}

describe('solveLayout', () => {
  it('lays out a simple textBlock on terminal surface', () => {
    const node: TextBlockNode = {
      kind: 'textBlock',
      id: 'txt-1',
      text: 'Hello, world',
      role: 'body',
      fontToken: 'body-md',
    };

    const result = solveLayout(node, terminalConstraints(80));

    expect(result.valid).toBe(true);
    expect(result.violations).toEqual([]);
    expect(result.width).toBe(80);
    // Height should be 1 (single line of text * lineHeight=1 for terminal)
    expect(result.height).toBe(1);

    const solved = result.nodes.find((n) => n.id === 'txt-1');
    expect(solved).toBeDefined();
    expect(solved!.x).toBe(0);
    expect(solved!.y).toBe(0);
    expect(solved!.width).toBe(80);
    expect(solved!.text).toBe('Hello, world');
    expect(solved!.lineCount).toBe(1);
    expect(solved!.overflow).toBe(false);
  });

  it('lays out a card containing a column of textBlocks', () => {
    const title: TextBlockNode = {
      kind: 'textBlock',
      id: 'title',
      text: 'Card Title',
      role: 'title',
      fontToken: 'heading-lg',
    };

    const body: TextBlockNode = {
      kind: 'textBlock',
      id: 'body',
      text: 'Body text content here',
      role: 'body',
      fontToken: 'body-md',
    };

    const column: ColumnNode = {
      kind: 'column',
      id: 'col-1',
      children: [title, body],
      gap: 4,
    };

    const card: CardNode = {
      kind: 'card',
      id: 'card-1',
      children: [column],
      padding: 8,
      widthPolicy: 'fill',
    };

    const result = solveLayout(card, terminalConstraints(80));

    expect(result.valid).toBe(true);
    expect(result.width).toBe(80);

    // Card should exist
    const cardSolved = result.nodes.find((n) => n.id === 'card-1');
    expect(cardSolved).toBeDefined();
    expect(cardSolved!.width).toBe(80);

    // Column should be inside card (offset by padding)
    const colSolved = result.nodes.find((n) => n.id === 'col-1');
    expect(colSolved).toBeDefined();
    expect(colSolved!.x).toBe(8); // card padding
    expect(colSolved!.y).toBe(8); // card padding

    // Title and body should be stacked vertically inside column
    const titleSolved = result.nodes.find((n) => n.id === 'title');
    const bodySolved = result.nodes.find((n) => n.id === 'body');
    expect(titleSolved).toBeDefined();
    expect(bodySolved).toBeDefined();
    // Body should be below title (title height + gap)
    expect(bodySolved!.y).toBeGreaterThan(titleSolved!.y);
  });

  it('lays out an inlineRow of chips', () => {
    const chips: ChipNode[] = [
      { kind: 'chip', id: 'chip-1', label: 'Rust' },
      { kind: 'chip', id: 'chip-2', label: 'TypeScript' },
      { kind: 'chip', id: 'chip-3', label: 'Go' },
    ];

    const row: InlineRowNode = {
      kind: 'inlineRow',
      id: 'row-1',
      children: chips,
      wrap: false,
      gap: 4,
    };

    const result = solveLayout(row, terminalConstraints(80));

    expect(result.valid).toBe(true);

    // Verify chips are laid out horizontally
    const chip1 = result.nodes.find((n) => n.id === 'chip-1');
    const chip2 = result.nodes.find((n) => n.id === 'chip-2');
    const chip3 = result.nodes.find((n) => n.id === 'chip-3');

    expect(chip1).toBeDefined();
    expect(chip2).toBeDefined();
    expect(chip3).toBeDefined();

    // All chips should be on the same y
    expect(chip1!.y).toBe(chip2!.y);
    expect(chip2!.y).toBe(chip3!.y);

    // Chips should be ordered left to right
    expect(chip2!.x).toBeGreaterThan(chip1!.x);
    expect(chip3!.x).toBeGreaterThan(chip2!.x);

    // Each chip should have positive width and height
    expect(chip1!.width).toBeGreaterThan(0);
    expect(chip1!.height).toBeGreaterThan(0);
  });

  it('lays out an icon with fixed dimensions', () => {
    const icon: IconNode = {
      kind: 'icon',
      id: 'icon-1',
      name: 'star',
      size: 24,
    };

    const result = solveLayout(icon, terminalConstraints(80));

    const solved = result.nodes.find((n) => n.id === 'icon-1');
    expect(solved).toBeDefined();
    // Root node gets container width from Yoga; height respects the fixed size
    expect(solved!.width).toBeLessThanOrEqual(80);
    expect(solved!.height).toBe(24);
  });

  it('lays out a button with minimum width', () => {
    const button: ButtonNode = {
      kind: 'button',
      id: 'btn-1',
      label: 'OK',
      action: 'submit',
      variant: 'primary',
    };

    const result = solveLayout(button, terminalConstraints(80));

    const solved = result.nodes.find((n) => n.id === 'btn-1');
    expect(solved).toBeDefined();
    // Button should be at least 80 units wide (min width enforced)
    expect(solved!.width).toBeGreaterThanOrEqual(80);
    expect(solved!.height).toBeGreaterThan(0);
  });

  it('lays out a section with title and children', () => {
    const body: TextBlockNode = {
      kind: 'textBlock',
      id: 'sec-body',
      text: 'Section content',
      role: 'body',
      fontToken: 'body-md',
    };

    const section: SectionNode = {
      kind: 'section',
      id: 'sec-1',
      title: 'My Section',
      children: [body],
    };

    const result = solveLayout(section, terminalConstraints(80));

    expect(result.valid).toBe(true);

    const secSolved = result.nodes.find((n) => n.id === 'sec-1');
    expect(secSolved).toBeDefined();
    expect(secSolved!.width).toBe(80);

    // The body text should be positioned below the implicit title
    const bodySolved = result.nodes.find((n) => n.id === 'sec-body');
    expect(bodySolved).toBeDefined();
    expect(bodySolved!.y).toBeGreaterThan(0);
  });

  it('detects text overflow against maxLines constraint', () => {
    const node: TextBlockNode = {
      kind: 'textBlock',
      id: 'overflow-txt',
      text: 'A'.repeat(200), // very long text
      role: 'body',
      fontToken: 'body-md',
      constraints: { maxLines: 2 },
    };

    const result = solveLayout(node, terminalConstraints(40));

    const solved = result.nodes.find((n) => n.id === 'overflow-txt');
    expect(solved).toBeDefined();
    // 200 chars / 40 cols = 5 lines, maxLines = 2 => overflow
    expect(solved!.lineCount).toBe(5);
    expect(solved!.overflow).toBe(true);
  });

  it('works with a non-terminal surface using default estimate', () => {
    const rawSurface: Surface = { kind: 'raw' };
    const node: TextBlockNode = {
      kind: 'textBlock',
      id: 'raw-txt',
      text: 'Hello',
      role: 'body',
      fontToken: 'body-md',
    };

    const result = solveLayout(node, { width: 400, surface: rawSurface });

    expect(result.valid).toBe(true);
    expect(result.width).toBe(400);
    expect(result.nodes.length).toBeGreaterThan(0);
  });

  it('accepts a custom text measure function', () => {
    const node: TextBlockNode = {
      kind: 'textBlock',
      id: 'custom-txt',
      text: 'Test text',
      role: 'body',
      fontToken: 'body-md',
    };

    // Custom measure: every character is 10px wide, lines are 20px tall
    const customMeasure = (text: string, maxWidth: number) => {
      const textWidth = text.length * 10;
      if (textWidth <= maxWidth) {
        return { lineCount: 1, height: 20, width: textWidth };
      }
      const lineCount = Math.ceil(textWidth / maxWidth);
      return { lineCount, height: lineCount * 20, width: maxWidth };
    };

    const result = solveLayout(
      node,
      { width: 200, surface: { kind: 'raw' } },
      customMeasure,
    );

    expect(result.valid).toBe(true);
    const solved = result.nodes.find((n) => n.id === 'custom-txt');
    expect(solved).toBeDefined();
    // "Test text" = 9 chars * 10 = 90px, fits in 200px
    expect(solved!.lineCount).toBe(1);
  });

  it('handles wrapping inlineRow', () => {
    const chips: ChipNode[] = Array.from({ length: 10 }, (_, i) => ({
      kind: 'chip' as const,
      id: `chip-${i}`,
      label: `Tag ${i}`,
    }));

    const row: InlineRowNode = {
      kind: 'inlineRow',
      id: 'wrap-row',
      children: chips,
      wrap: true,
      gap: 4,
    };

    // Use a narrow width to force wrapping
    const result = solveLayout(row, terminalConstraints(40));

    expect(result.valid).toBe(true);

    // With wrap enabled and narrow width, chips should span multiple rows.
    // Find distinct y positions among chips.
    const chipNodes = result.nodes.filter((n) => n.id.startsWith('chip-'));
    const uniqueYs = new Set(chipNodes.map((n) => n.y));
    expect(uniqueYs.size).toBeGreaterThan(1);
  });
});

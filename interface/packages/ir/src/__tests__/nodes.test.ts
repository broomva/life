import { describe, expect, it } from 'vitest';
import type {
  ButtonNode,
  CardNode,
  ChipNode,
  TextBlockNode,
  UINode,
} from '../nodes.js';

describe('UINode types', () => {
  it('creates a TextBlockNode with required fields', () => {
    const node: TextBlockNode = {
      kind: 'textBlock',
      id: 'txt-1',
      text: 'Hello, world',
      role: 'title',
      fontToken: 'heading-lg',
    };

    expect(node.kind).toBe('textBlock');
    expect(node.id).toBe('txt-1');
    expect(node.text).toBe('Hello, world');
    expect(node.role).toBe('title');
    expect(node.fontToken).toBe('heading-lg');
    expect(node.constraints).toBeUndefined();
  });

  it('creates a TextBlockNode with optional constraints', () => {
    const node: TextBlockNode = {
      kind: 'textBlock',
      id: 'txt-2',
      text: 'Constrained text',
      role: 'body',
      fontToken: 'body-md',
      constraints: {
        maxLines: 3,
        maxWidth: 400,
        overflowPolicy: 'ellipsis',
      },
    };

    expect(node.constraints?.maxLines).toBe(3);
    expect(node.constraints?.overflowPolicy).toBe('ellipsis');
  });

  it('creates a CardNode with children', () => {
    const title: TextBlockNode = {
      kind: 'textBlock',
      id: 'card-title',
      text: 'Card Title',
      role: 'title',
      fontToken: 'heading-md',
    };

    const chip: ChipNode = {
      kind: 'chip',
      id: 'chip-1',
      label: 'Active',
      priority: 10,
    };

    const card: CardNode = {
      kind: 'card',
      id: 'card-1',
      children: [title, chip],
      padding: 16,
      widthPolicy: 'fill',
      constraints: {
        maxWidth: 600,
        density: 'normal',
      },
    };

    expect(card.kind).toBe('card');
    expect(card.children).toHaveLength(2);
    expect(card.children[0].kind).toBe('textBlock');
    expect(card.children[1].kind).toBe('chip');
    expect(card.padding).toBe(16);
    expect(card.widthPolicy).toBe('fill');
    expect(card.constraints?.maxWidth).toBe(600);
  });

  it('narrows UINode discriminated union on kind', () => {
    const nodes: UINode[] = [
      {
        kind: 'textBlock',
        id: 'n1',
        text: 'Hello',
        role: 'body',
        fontToken: 'body-md',
      },
      {
        kind: 'button',
        id: 'n2',
        label: 'Click me',
        action: 'submit',
        variant: 'primary',
      },
      {
        kind: 'chip',
        id: 'n3',
        label: 'Tag',
      },
    ];

    const results: string[] = [];

    for (const node of nodes) {
      switch (node.kind) {
        case 'textBlock':
          // TypeScript narrows to TextBlockNode here
          results.push(`text:${node.text}`);
          break;
        case 'button':
          // TypeScript narrows to ButtonNode here
          results.push(`button:${node.action}`);
          break;
        case 'chip':
          // TypeScript narrows to ChipNode here
          results.push(`chip:${node.label}`);
          break;
        default:
          results.push('other');
      }
    }

    expect(results).toEqual(['text:Hello', 'button:submit', 'chip:Tag']);
  });

  it('handles all node kinds in the union', () => {
    // Exhaustiveness helper: ensures every kind is covered
    function assertNodeKind(node: UINode): string {
      switch (node.kind) {
        case 'textBlock':
          return node.role;
        case 'inlineRow':
          return `row:${node.children.length}`;
        case 'card':
          return `card:${node.widthPolicy}`;
        case 'column':
          return `col:${node.gap}`;
        case 'chip':
          return `chip:${node.label}`;
        case 'icon':
          return `icon:${node.name}`;
        case 'button':
          return `btn:${node.action}`;
        case 'section':
          return `section:${node.title}`;
      }
    }

    const icon: UINode = { kind: 'icon', id: 'i1', name: 'star', size: 24 };
    expect(assertNodeKind(icon)).toBe('icon:star');

    const section: UINode = {
      kind: 'section',
      id: 's1',
      title: 'Section One',
      children: [],
    };
    expect(assertNodeKind(section)).toBe('section:Section One');
  });
});

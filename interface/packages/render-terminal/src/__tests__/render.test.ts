import { describe, expect, it } from 'vitest';
import type { SolvedLayout } from '@life/ikr-ir';
import { renderToTerminal, wrapText } from '../render.js';

describe('wrapText', () => {
  it('returns empty array for zero width', () => {
    expect(wrapText('hello', 0)).toEqual([]);
  });

  it('returns single line when text fits', () => {
    expect(wrapText('hello world', 20)).toEqual(['hello world']);
  });

  it('wraps long text at word boundaries', () => {
    expect(wrapText('hello world foo bar', 11)).toEqual(['hello world', 'foo bar']);
  });

  it('handles single word longer than width', () => {
    // A single word that exceeds width still gets placed on its own line
    expect(wrapText('superlongword short', 5)).toEqual(['superlongword', 'short']);
  });

  it('handles multiple spaces between words', () => {
    // split(/\s+/) collapses multiple spaces
    expect(wrapText('hello    world', 20)).toEqual(['hello world']);
  });

  it('handles empty string', () => {
    expect(wrapText('', 10)).toEqual([]);
  });
});

describe('renderToTerminal', () => {
  it('renders a simple text node', () => {
    const layout: SolvedLayout = {
      valid: true,
      width: 20,
      height: 3,
      nodes: [
        {
          id: 'txt-1',
          x: 0,
          y: 0,
          width: 20,
          height: 1,
          overflow: false,
          text: 'Hello, terminal!',
        },
      ],
      violations: [],
    };

    const output = renderToTerminal(layout);
    const lines = output.split('\n');
    expect(lines[0]).toContain('Hello, terminal!');
    expect(lines).toHaveLength(3);
  });

  it('renders text at offset position', () => {
    const layout: SolvedLayout = {
      valid: true,
      width: 30,
      height: 3,
      nodes: [
        {
          id: 'txt-1',
          x: 5,
          y: 1,
          width: 20,
          height: 1,
          overflow: false,
          text: 'Offset text',
        },
      ],
      violations: [],
    };

    const output = renderToTerminal(layout);
    const lines = output.split('\n');
    // Row 0 should be all spaces
    expect(lines[0].trim()).toBe('');
    // Row 1 should have text starting at col 5
    expect(lines[1].indexOf('Offset text')).toBe(5);
  });

  it('wraps text within node width', () => {
    const layout: SolvedLayout = {
      valid: true,
      width: 10,
      height: 3,
      nodes: [
        {
          id: 'txt-1',
          x: 0,
          y: 0,
          width: 10,
          height: 3,
          overflow: false,
          text: 'hello world foo',
        },
      ],
      violations: [],
    };

    const output = renderToTerminal(layout);
    const lines = output.split('\n');
    expect(lines[0].trimEnd()).toBe('hello');
    expect(lines[1].trimEnd()).toBe('world foo');
  });

  it('renders multiple nodes', () => {
    const layout: SolvedLayout = {
      valid: true,
      width: 20,
      height: 3,
      nodes: [
        {
          id: 'a',
          x: 0,
          y: 0,
          width: 20,
          height: 1,
          overflow: false,
          text: 'First',
        },
        {
          id: 'b',
          x: 0,
          y: 2,
          width: 20,
          height: 1,
          overflow: false,
          text: 'Second',
        },
      ],
      violations: [],
    };

    const output = renderToTerminal(layout);
    const lines = output.split('\n');
    expect(lines[0]).toContain('First');
    expect(lines[2]).toContain('Second');
  });

  it('renders children recursively', () => {
    const layout: SolvedLayout = {
      valid: true,
      width: 20,
      height: 3,
      nodes: [
        {
          id: 'parent',
          x: 0,
          y: 0,
          width: 20,
          height: 3,
          overflow: false,
          children: [
            {
              id: 'child',
              x: 2,
              y: 1,
              width: 10,
              height: 1,
              overflow: false,
              text: 'Nested',
            },
          ],
        },
      ],
      violations: [],
    };

    const output = renderToTerminal(layout);
    const lines = output.split('\n');
    expect(lines[1].indexOf('Nested')).toBe(2);
  });

  it('respects explicit cols and rows override', () => {
    const layout: SolvedLayout = {
      valid: true,
      width: 100,
      height: 50,
      nodes: [
        {
          id: 'txt',
          x: 0,
          y: 0,
          width: 10,
          height: 1,
          overflow: false,
          text: 'Hi',
        },
      ],
      violations: [],
    };

    const output = renderToTerminal(layout, 10, 2);
    const lines = output.split('\n');
    expect(lines).toHaveLength(2);
    expect(lines[0].length).toBeLessThanOrEqual(10);
  });
});

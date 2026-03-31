import type { SolvedLayout, SolvedNode } from '@life/ikr-ir';
import { TerminalBuffer } from './buffer.js';
import { moveCursor, resetStyle } from './ansi.js';

export function renderToTerminal(solved: SolvedLayout, cols?: number, rows?: number): string {
  const w = cols ?? Math.ceil(solved.width);
  const h = rows ?? Math.ceil(solved.height);
  const buffer = new TerminalBuffer(w, h);

  for (const node of solved.nodes) {
    renderNode(buffer, node);
  }

  return buffer.toString();
}

function renderNode(buffer: TerminalBuffer, node: SolvedNode): void {
  const top = Math.round(node.y);
  const left = Math.round(node.x);
  const width = Math.round(node.width);
  const height = Math.round(node.height);

  if (node.text) {
    // Render text, wrapping within the node's width
    const lines = wrapText(node.text, width);
    for (let i = 0; i < lines.length && top + i < buffer.rows; i++) {
      buffer.writeText(top + i, left, lines[i]);
    }
  }

  if (node.children) {
    for (const child of node.children) {
      renderNode(buffer, child);
    }
  }
}

export function wrapText(text: string, width: number): string[] {
  if (width <= 0) return [];
  const words = text.split(/\s+/);
  const lines: string[] = [];
  let current = '';

  for (const word of words) {
    if (current.length === 0) {
      current = word;
    } else if (current.length + 1 + word.length <= width) {
      current += ' ' + word;
    } else {
      lines.push(current);
      current = word;
    }
  }
  if (current) lines.push(current);
  return lines;
}

/** Minimal writable stream interface compatible with NodeJS.WriteStream */
export type TerminalStream = {
  columns?: number;
  rows?: number;
  write(data: string): boolean;
};

export function createReactiveTerminalRenderer(stream: TerminalStream) {
  let prevBuffer: TerminalBuffer | null = null;

  return function update(solved: SolvedLayout): void {
    const cols = stream.columns ?? 80;
    const rows = stream.rows ?? 24;
    const buffer = new TerminalBuffer(cols, rows);

    for (const node of solved.nodes) {
      renderNode(buffer, node);
    }

    if (prevBuffer) {
      const patches = buffer.diff(prevBuffer);
      for (const patch of patches) {
        const styled = patch.style ? `${patch.style}${patch.char}${resetStyle()}` : patch.char;
        stream.write(`${moveCursor(patch.row, patch.col)}${styled}`);
      }
    } else {
      stream.write(buffer.toString());
    }

    prevBuffer = buffer;
  };
}

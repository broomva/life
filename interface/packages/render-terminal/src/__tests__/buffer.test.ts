import { describe, expect, it } from 'vitest';
import { TerminalBuffer } from '../buffer.js';

describe('TerminalBuffer', () => {
  it('creates buffer with correct dimensions', () => {
    const buf = new TerminalBuffer(10, 5);
    expect(buf.cols).toBe(10);
    expect(buf.rows).toBe(5);
  });

  it('initializes all cells to spaces', () => {
    const buf = new TerminalBuffer(3, 2);
    const output = buf.toString();
    expect(output).toBe('   \n   ');
  });

  it('sets a character at a position', () => {
    const buf = new TerminalBuffer(5, 3);
    buf.set(1, 2, 'X');
    const lines = buf.toString().split('\n');
    expect(lines[1][2]).toBe('X');
  });

  it('ignores out-of-bounds set calls', () => {
    const buf = new TerminalBuffer(3, 3);
    buf.set(-1, 0, 'X');
    buf.set(0, -1, 'X');
    buf.set(3, 0, 'X');
    buf.set(0, 3, 'X');
    // All cells should still be spaces
    expect(buf.toString()).toBe('   \n   \n   ');
  });

  it('writeText places characters correctly', () => {
    const buf = new TerminalBuffer(10, 1);
    buf.writeText(0, 2, 'Hello');
    const output = buf.toString();
    expect(output).toBe('  Hello   ');
  });

  it('writeText truncates at buffer boundary', () => {
    const buf = new TerminalBuffer(5, 1);
    buf.writeText(0, 3, 'Hello');
    const output = buf.toString();
    expect(output).toBe('   He');
  });

  it('writeText applies style to each character', () => {
    const buf = new TerminalBuffer(5, 1);
    buf.writeText(0, 0, 'AB', '\x1b[1m');
    const output = buf.toString();
    // Each styled char: style + char + reset
    expect(output).toContain('\x1b[1m');
    expect(output).toContain('\x1b[0m');
    expect(output).toContain('A');
    expect(output).toContain('B');
  });

  it('drawBox draws borders correctly', () => {
    const buf = new TerminalBuffer(6, 4);
    buf.drawBox(0, 0, 6, 4);
    const lines = buf.toString().split('\n');
    expect(lines[0]).toBe('┌────┐');
    expect(lines[1]).toBe('│    │');
    expect(lines[2]).toBe('│    │');
    expect(lines[3]).toBe('└────┘');
  });

  it('drawBox with offset position', () => {
    const buf = new TerminalBuffer(8, 5);
    buf.drawBox(1, 1, 4, 3);
    const lines = buf.toString().split('\n');
    // Row 0 is all spaces
    expect(lines[0]).toBe('        ');
    // Row 1: space + top border
    expect(lines[1]).toBe(' ┌──┐   ');
    // Row 2: space + sides
    expect(lines[2]).toBe(' │  │   ');
    // Row 3: space + bottom border
    expect(lines[3]).toBe(' └──┘   ');
  });

  it('diff returns only changed cells', () => {
    const a = new TerminalBuffer(3, 2);
    const b = new TerminalBuffer(3, 2);

    a.writeText(0, 0, 'ABC');
    b.writeText(0, 0, 'AXC');

    const patches = a.diff(b);
    // Only position (0,1) differs: A=B, B!=X, C=C
    expect(patches).toHaveLength(1);
    expect(patches[0]).toEqual({ row: 0, col: 1, char: 'B', style: undefined });
  });

  it('diff detects style changes', () => {
    const a = new TerminalBuffer(3, 1);
    const b = new TerminalBuffer(3, 1);

    a.writeText(0, 0, 'A', '\x1b[1m');
    b.writeText(0, 0, 'A'); // same char, no style

    const patches = a.diff(b);
    expect(patches).toHaveLength(1);
    expect(patches[0].style).toBe('\x1b[1m');
  });

  it('diff returns empty array for identical buffers', () => {
    const a = new TerminalBuffer(4, 2);
    const b = new TerminalBuffer(4, 2);

    a.writeText(0, 0, 'test');
    b.writeText(0, 0, 'test');

    const patches = a.diff(b);
    expect(patches).toHaveLength(0);
  });
});

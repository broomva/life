export type Cell = {
  char: string;
  style?: string; // ANSI style prefix
};

export class TerminalBuffer {
  readonly cols: number;
  readonly rows: number;
  private cells: Cell[][];

  constructor(cols: number, rows: number) {
    this.cols = cols;
    this.rows = rows;
    this.cells = Array.from({ length: rows }, () =>
      Array.from({ length: cols }, () => ({ char: ' ' }))
    );
  }

  set(row: number, col: number, char: string, style?: string): void {
    if (row >= 0 && row < this.rows && col >= 0 && col < this.cols) {
      this.cells[row][col] = { char, style };
    }
  }

  writeText(row: number, col: number, text: string, style?: string): void {
    for (let i = 0; i < text.length && col + i < this.cols; i++) {
      this.set(row, col + i, text[i], style);
    }
  }

  drawBox(top: number, left: number, width: number, height: number, style?: string): void {
    // Top edge
    this.set(top, left, '┌', style);
    for (let c = 1; c < width - 1; c++) this.set(top, left + c, '─', style);
    this.set(top, left + width - 1, '┐', style);
    // Bottom edge
    this.set(top + height - 1, left, '└', style);
    for (let c = 1; c < width - 1; c++) this.set(top + height - 1, left + c, '─', style);
    this.set(top + height - 1, left + width - 1, '┘', style);
    // Sides
    for (let r = 1; r < height - 1; r++) {
      this.set(top + r, left, '│', style);
      this.set(top + r, left + width - 1, '│', style);
    }
  }

  toString(): string {
    return this.cells
      .map(row =>
        row.map(cell => (cell.style ? `${cell.style}${cell.char}\x1b[0m` : cell.char)).join('')
      )
      .join('\n');
  }

  /** Diff against another buffer, returning only changed positions */
  diff(other: TerminalBuffer): Array<{ row: number; col: number; char: string; style?: string }> {
    const patches: Array<{ row: number; col: number; char: string; style?: string }> = [];
    for (let r = 0; r < this.rows; r++) {
      for (let c = 0; c < this.cols; c++) {
        const a = this.cells[r][c];
        const b = other.cells[r]?.[c];
        if (!b || a.char !== b.char || a.style !== b.style) {
          patches.push({ row: r, col: c, char: a.char, style: a.style });
        }
      }
    }
    return patches;
  }
}

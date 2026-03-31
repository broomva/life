// ANSI escape codes for terminal rendering
export const ESC = '\x1b[';

export function moveCursor(row: number, col: number): string {
  return `${ESC}${row + 1};${col + 1}H`; // ANSI is 1-indexed
}

export function clearScreen(): string {
  return `${ESC}2J${ESC}H`;
}

export function resetStyle(): string {
  return `${ESC}0m`;
}

export function bold(text: string): string {
  return `${ESC}1m${text}${ESC}22m`;
}

export function dim(text: string): string {
  return `${ESC}2m${text}${ESC}22m`;
}

export function fg256(color: number, text: string): string {
  return `${ESC}38;5;${color}m${text}${ESC}39m`;
}

export function bg256(color: number, text: string): string {
  return `${ESC}48;5;${color}m${text}${ESC}49m`;
}

// Box drawing characters
export const BOX = {
  topLeft: '┌',
  topRight: '┐',
  bottomLeft: '└',
  bottomRight: '┘',
  horizontal: '─',
  vertical: '│',
} as const;

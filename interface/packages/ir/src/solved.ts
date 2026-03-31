import type { Violation } from './violations.js';

export type SolvedLayout = {
  valid: boolean;
  width: number;
  height: number;
  nodes: SolvedNode[];
  violations: Violation[];
};

export type SolvedNode = {
  id: string;
  x: number;
  y: number;
  width: number;
  height: number;
  lineCount?: number;
  overflow: boolean;
  text?: string;
  children?: SolvedNode[];
};

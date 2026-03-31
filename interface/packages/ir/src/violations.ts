export type ViolationSeverity = 'error' | 'warning';

export type Violation = {
  nodeId: string;
  rule: string;
  severity: ViolationSeverity;
  actual: number;
  limit: number;
  repairOptions: RepairStrategy[];
};

export type RepairStrategy =
  | { kind: 'summarize_text'; nodeId: string; targetChars: number }
  | { kind: 'collapse_chips'; nodeId: string; maxVisible: number }
  | { kind: 'widen_container'; nodeId: string; targetWidth: number }
  | { kind: 'switch_density'; nodeId: string; density: 'compact' }
  | { kind: 'reduce_font_token'; nodeId: string; token: string }
  | { kind: 'increase_max_lines'; nodeId: string; lines: number }
  | { kind: 'hide_node'; nodeId: string };

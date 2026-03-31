export type FontMetrics = {
  font: string;
  lineHeight: number;
};

export type Surface =
  | { kind: 'dom'; fontMetrics: FontMetrics }
  | { kind: 'canvas'; fontMetrics: FontMetrics }
  | { kind: 'terminal'; cols: number; rows: number; monoWidth: 1 }
  | { kind: 'pdf'; pageWidth: number; pageHeight: number }
  | { kind: 'raw' };

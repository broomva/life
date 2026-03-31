import type { Surface } from './surface.js';

export type OverflowPolicy = 'clip' | 'ellipsis' | 'summarize' | 'reflow';
export type Density = 'compact' | 'normal' | 'spacious';

export type TextConstraints = {
  maxLines?: number;
  maxWidth?: number;
  overflowPolicy?: OverflowPolicy;
};

export type BoxConstraints = {
  minWidth?: number;
  maxWidth?: number;
  minHeight?: number;
  maxHeight?: number;
  density?: Density;
};

export type LayoutConstraints = {
  width: number;
  height?: number;
  lineHeight?: number;
  surface: Surface;
};

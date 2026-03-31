import type { BoxConstraints, TextConstraints } from './constraints.js';

export type TextRole = 'title' | 'subtitle' | 'body' | 'caption' | 'label' | 'code';

export type TextBlockNode = {
  kind: 'textBlock';
  id: string;
  text: string;
  role: TextRole;
  fontToken: string;
  constraints?: TextConstraints;
};

export type InlineRowNode = {
  kind: 'inlineRow';
  id: string;
  children: UINode[];
  wrap: boolean;
  gap: number;
  collapsePolicy?: 'hide-low-priority' | 'overflow-chip' | 'wrap';
};

export type CardNode = {
  kind: 'card';
  id: string;
  children: UINode[];
  padding: number;
  widthPolicy: 'fixed' | 'shrinkWrap' | 'fill';
  constraints?: BoxConstraints;
};

export type ColumnNode = {
  kind: 'column';
  id: string;
  children: UINode[];
  gap: number;
};

export type ChipNode = {
  kind: 'chip';
  id: string;
  label: string;
  priority?: number;
};

export type IconNode = {
  kind: 'icon';
  id: string;
  name: string;
  size: number;
};

export type ButtonNode = {
  kind: 'button';
  id: string;
  label: string;
  action: string;
  variant?: 'primary' | 'secondary' | 'ghost';
};

export type SectionNode = {
  kind: 'section';
  id: string;
  title: string;
  children: UINode[];
};

export type UINode =
  | TextBlockNode
  | InlineRowNode
  | CardNode
  | ColumnNode
  | ChipNode
  | IconNode
  | ButtonNode
  | SectionNode;

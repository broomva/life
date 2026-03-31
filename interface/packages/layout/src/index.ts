export { solveLayout } from './solve.js';
export { buildYogaTree, extractSolvedNodes } from './box-layout.js';
export type { TextMeasureFn } from './box-layout.js';
export { measureMonoText, monoStringWidth } from './text-mono.js';
// Note: text-browser is NOT exported from the barrel since it requires
// a Canvas-capable environment. Import it directly when needed:
//   import { measureBrowserText } from '@life/ikr-layout/text-browser';

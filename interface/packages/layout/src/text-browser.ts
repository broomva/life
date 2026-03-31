/**
 * Browser text measurement backed by @chenglou/pretext.
 *
 * Requires a Canvas-capable environment (browser or node-canvas).
 * Do NOT import from the package entry point — import directly when
 * a browser/Canvas context is available.
 */
import { prepare, layout as pretextLayout } from '@chenglou/pretext';
import type { FontMetrics } from '@life/ikr-ir';
import type { PreparedText } from '@chenglou/pretext';

// Cache prepared text handles to avoid re-analyzing the same string
const preparedCache = new Map<string, PreparedText>();

function getCacheKey(text: string, font: string): string {
  return `${font}::${text}`;
}

export function prepareText(text: string, font: string): PreparedText {
  const key = getCacheKey(text, font);
  let prepared = preparedCache.get(key);
  if (!prepared) {
    prepared = prepare(text, font);
    preparedCache.set(key, prepared);
  }
  return prepared;
}

export function measureBrowserText(
  text: string,
  fontMetrics: FontMetrics,
  maxWidth: number,
): { lineCount: number; height: number; width: number } {
  const prepared = prepareText(text, fontMetrics.font);
  const result = pretextLayout(prepared, maxWidth, fontMetrics.lineHeight);
  return {
    lineCount: result.lineCount,
    height: result.height,
    width: maxWidth, // Pretext layout() returns lineCount+height but not actual max line width
  };
}

export function clearTextCache(): void {
  preparedCache.clear();
}

/**
 * Monospace text measurement for terminal surfaces.
 *
 * Width = number of character cells (CJK chars take 2 cells each).
 * Uses a simple heuristic since we can't import a native wcwidth in pure TS.
 */

/**
 * Calculate the display width of a string in a monospace terminal.
 * CJK characters take 2 columns. Control characters take 0.
 */
export function monoStringWidth(str: string): number {
  let width = 0;
  for (const char of str) {
    const code = char.codePointAt(0);
    if (code === undefined) continue;
    // Control chars: C0 + DEL + C1
    if (code < 32 || (code >= 0x7f && code < 0xa0)) continue;
    if (isFullWidth(code)) {
      width += 2;
    } else {
      width += 1;
    }
  }
  return width;
}

/**
 * Measure text in a monospace environment.
 * Returns the number of lines required and the pixel dimensions
 * (where 1 pixel = 1 character cell for terminal surfaces).
 */
export function measureMonoText(
  text: string,
  maxWidth: number,
): { lineCount: number; height: number; width: number } {
  if (maxWidth <= 0) {
    return { lineCount: 1, height: 1, width: 0 };
  }

  const charWidth = monoStringWidth(text);
  if (charWidth <= maxWidth) {
    return { lineCount: 1, height: 1, width: charWidth };
  }

  const lineCount = Math.ceil(charWidth / maxWidth);
  return { lineCount, height: lineCount, width: maxWidth };
}

function isFullWidth(code: number): boolean {
  return (
    (code >= 0x1100 && code <= 0x115f) || // Hangul Jamo
    (code >= 0x2e80 && code <= 0x3247) || // CJK Radicals..Enclosed CJK
    (code >= 0x3250 && code <= 0x4dbf) || // CJK Compatibility..CJK Unified
    (code >= 0x4e00 && code <= 0xa4cf) || // CJK Unified Ideographs..Yi
    (code >= 0xac00 && code <= 0xd7a3) || // Hangul Syllables
    (code >= 0xf900 && code <= 0xfaff) || // CJK Compatibility Ideographs
    (code >= 0xfe10 && code <= 0xfe6f) || // Vertical forms..Small forms
    (code >= 0xff01 && code <= 0xff60) || // Fullwidth forms
    (code >= 0xffe0 && code <= 0xffe6) || // Fullwidth signs
    (code >= 0x20000 && code <= 0x2fffd) || // CJK Extension B+
    (code >= 0x30000 && code <= 0x3fffd) // CJK Extension G+
  );
}

import { describe, expect, it } from 'vitest';
import { monoStringWidth, measureMonoText } from '../text-mono.js';

describe('monoStringWidth', () => {
  it('measures ASCII text as 1 cell per character', () => {
    expect(monoStringWidth('hello')).toBe(5);
    expect(monoStringWidth('abc 123')).toBe(7);
  });

  it('returns 0 for empty string', () => {
    expect(monoStringWidth('')).toBe(0);
  });

  it('ignores control characters', () => {
    // Tab (\t = 0x09), newline (\n = 0x0a), carriage return (\r = 0x0d)
    expect(monoStringWidth('\t')).toBe(0);
    expect(monoStringWidth('\n')).toBe(0);
    expect(monoStringWidth('a\tb')).toBe(2);
  });

  it('counts CJK characters as 2 cells', () => {
    // Chinese characters (CJK Unified Ideographs: U+4E00..U+9FFF)
    expect(monoStringWidth('\u4e00')).toBe(2); // 一
    expect(monoStringWidth('\u4f60\u597d')).toBe(4); // 你好
  });

  it('counts fullwidth forms as 2 cells', () => {
    // Fullwidth Latin (U+FF01..U+FF60)
    expect(monoStringWidth('\uff21')).toBe(2); // Ａ (fullwidth A)
  });

  it('handles mixed ASCII and CJK', () => {
    // "Hello世界" = 5 ASCII + 2 CJK * 2 = 9
    expect(monoStringWidth('Hello\u4e16\u754c')).toBe(9);
  });

  it('counts Hangul syllables as 2 cells', () => {
    // Hangul Syllables (U+AC00..U+D7A3)
    expect(monoStringWidth('\uac00')).toBe(2); // 가
    expect(monoStringWidth('\uac00\uac01')).toBe(4); // 가각
  });
});

describe('measureMonoText', () => {
  it('returns single line when text fits', () => {
    const result = measureMonoText('hello', 80);
    expect(result).toEqual({ lineCount: 1, height: 1, width: 5 });
  });

  it('wraps text that exceeds maxWidth', () => {
    // "hello world" = 11 chars, maxWidth 5 => ceil(11/5) = 3 lines
    const result = measureMonoText('hello world', 5);
    expect(result.lineCount).toBe(3);
    expect(result.height).toBe(3);
    expect(result.width).toBe(5);
  });

  it('wraps CJK text accounting for double width', () => {
    // 3 CJK chars = 6 cells, maxWidth = 4 => ceil(6/4) = 2 lines
    const result = measureMonoText('\u4f60\u597d\u5417', 4);
    expect(result.lineCount).toBe(2);
    expect(result.height).toBe(2);
    expect(result.width).toBe(4);
  });

  it('handles empty text', () => {
    const result = measureMonoText('', 80);
    expect(result).toEqual({ lineCount: 1, height: 1, width: 0 });
  });

  it('handles maxWidth of 0 gracefully', () => {
    const result = measureMonoText('hello', 0);
    expect(result).toEqual({ lineCount: 1, height: 1, width: 0 });
  });

  it('handles exact fit at maxWidth boundary', () => {
    // "abcde" = 5 chars, maxWidth 5 => 1 line
    const result = measureMonoText('abcde', 5);
    expect(result).toEqual({ lineCount: 1, height: 1, width: 5 });
  });

  it('handles text one char over maxWidth', () => {
    // "abcdef" = 6 chars, maxWidth 5 => ceil(6/5) = 2 lines
    const result = measureMonoText('abcdef', 5);
    expect(result.lineCount).toBe(2);
    expect(result.height).toBe(2);
    expect(result.width).toBe(5);
  });
});

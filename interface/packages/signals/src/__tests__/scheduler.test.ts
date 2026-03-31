import { describe, it, expect, vi } from 'vitest';
import { scheduleLayout, flushLayout } from '../scheduler.js';

describe('scheduler', () => {
  describe('scheduleLayout', () => {
    it('batches multiple calls into one flush', async () => {
      const calls: number[] = [];
      scheduleLayout(() => calls.push(1));
      scheduleLayout(() => calls.push(2));
      scheduleLayout(() => calls.push(3));

      // Nothing should have run synchronously yet
      expect(calls).toEqual([]);

      // Flush synchronously
      flushLayout();

      expect(calls).toEqual([1, 2, 3]);
    });

    it('schedules to microtask in Node environment', async () => {
      const fn = vi.fn();
      scheduleLayout(fn);

      // Not yet called synchronously
      expect(fn).not.toHaveBeenCalled();

      // Wait for microtask
      await Promise.resolve();

      expect(fn).toHaveBeenCalledOnce();
    });
  });

  describe('flushLayout', () => {
    it('runs all pending functions synchronously', () => {
      const results: string[] = [];
      scheduleLayout(() => results.push('a'));
      scheduleLayout(() => results.push('b'));

      flushLayout();

      expect(results).toEqual(['a', 'b']);
    });

    it('does not double-execute after flush', async () => {
      const fn = vi.fn();
      scheduleLayout(fn);

      flushLayout();
      expect(fn).toHaveBeenCalledOnce();

      // Wait for any pending microtask to ensure no double-execution
      await Promise.resolve();
      await Promise.resolve();

      expect(fn).toHaveBeenCalledOnce();
    });

    it('is a no-op when nothing is pending', () => {
      // Should not throw
      flushLayout();
    });
  });
});

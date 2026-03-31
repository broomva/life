import { describe, it, expect, vi } from 'vitest';
import { createRoot, onCleanup, getOwner, runWithOwner } from '../ownership.js';
import type { Owner } from '../ownership.js';

describe('ownership', () => {
  describe('createRoot', () => {
    it('creates a root and runs the function', () => {
      const result = createRoot((dispose) => {
        return 42;
      });
      expect(result).toBe(42);
    });

    it('passes a dispose function to the callback', () => {
      let disposeFn: (() => void) | undefined;
      createRoot((dispose) => {
        disposeFn = dispose;
      });
      expect(typeof disposeFn).toBe('function');
    });
  });

  describe('onCleanup', () => {
    it('registers cleanup that runs on dispose', () => {
      const cleanup = vi.fn();
      createRoot((dispose) => {
        onCleanup(cleanup);
        expect(cleanup).not.toHaveBeenCalled();
        dispose();
        expect(cleanup).toHaveBeenCalledOnce();
      });
    });

    it('runs multiple cleanups in reverse order', () => {
      const order: number[] = [];
      createRoot((dispose) => {
        onCleanup(() => order.push(1));
        onCleanup(() => order.push(2));
        onCleanup(() => order.push(3));
        dispose();
      });
      expect(order).toEqual([3, 2, 1]);
    });

    it('does nothing if called outside an owner', () => {
      // Should not throw
      onCleanup(() => {});
    });
  });

  describe('nested roots', () => {
    it('disposes children before parent cleanups', () => {
      const order: string[] = [];
      createRoot((disposeOuter) => {
        onCleanup(() => order.push('parent'));

        createRoot((_disposeInner) => {
          onCleanup(() => order.push('child'));
        });

        disposeOuter();
      });
      // Children disposed first (depth-first), then parent cleanups
      expect(order).toEqual(['child', 'parent']);
    });

    it('disposes deeply nested children in depth-first order', () => {
      const order: string[] = [];
      createRoot((disposeRoot) => {
        onCleanup(() => order.push('root'));

        createRoot((_dispose) => {
          onCleanup(() => order.push('child-a'));

          createRoot((_dispose) => {
            onCleanup(() => order.push('grandchild-a'));
          });
        });

        createRoot((_dispose) => {
          onCleanup(() => order.push('child-b'));
        });

        disposeRoot();
      });
      expect(order).toEqual(['grandchild-a', 'child-a', 'child-b', 'root']);
    });
  });

  describe('getOwner', () => {
    it('returns null outside createRoot', () => {
      expect(getOwner()).toBeNull();
    });

    it('returns current owner inside createRoot', () => {
      createRoot((_dispose) => {
        const owner = getOwner();
        expect(owner).not.toBeNull();
        expect(owner!.parent).toBeNull(); // top-level root has no parent
      });
    });

    it('returns nested owner inside nested createRoot', () => {
      createRoot((_dispose) => {
        const outerOwner = getOwner();

        createRoot((_dispose) => {
          const innerOwner = getOwner();
          expect(innerOwner).not.toBeNull();
          expect(innerOwner!.parent).toBe(outerOwner);
        });
      });
    });
  });

  describe('runWithOwner', () => {
    it('restores previous owner after execution', () => {
      expect(getOwner()).toBeNull();

      createRoot((_dispose) => {
        const rootOwner = getOwner()!;

        const customOwner: Owner = {
          parent: null,
          children: [],
          cleanups: [],
        };

        runWithOwner(customOwner, () => {
          expect(getOwner()).toBe(customOwner);
        });

        // Should restore to rootOwner
        expect(getOwner()).toBe(rootOwner);
      });

      // Should restore to null outside
      expect(getOwner()).toBeNull();
    });

    it('restores owner even if fn throws', () => {
      createRoot((_dispose) => {
        const rootOwner = getOwner()!;
        const customOwner: Owner = {
          parent: null,
          children: [],
          cleanups: [],
        };

        try {
          runWithOwner(customOwner, () => {
            throw new Error('test error');
          });
        } catch {
          // expected
        }

        expect(getOwner()).toBe(rootOwner);
      });
    });

    it('returns the value from the function', () => {
      const owner: Owner = {
        parent: null,
        children: [],
        cleanups: [],
      };
      const result = runWithOwner(owner, () => 'hello');
      expect(result).toBe('hello');
    });
  });

  describe('dispose removes owner from parent children', () => {
    it('removes disposed child from parent children list', () => {
      createRoot((_disposeOuter) => {
        const parentOwner = getOwner()!;

        let innerDispose: (() => void) | undefined;
        createRoot((dispose) => {
          innerDispose = dispose;
        });

        expect(parentOwner.children.length).toBe(1);
        innerDispose!();
        expect(parentOwner.children.length).toBe(0);
      });
    });
  });
});

type ScheduledFn = () => void;

let pendingLayoutFns: ScheduledFn[] = [];
let rafScheduled = false;

const isBrowser = typeof globalThis.requestAnimationFrame === 'function';

function flush(): void {
  const fns = pendingLayoutFns;
  pendingLayoutFns = [];
  rafScheduled = false;
  for (const fn of fns) {
    fn();
  }
}

/**
 * Schedule a layout recomputation.
 * In browser: batched to requestAnimationFrame.
 * In Node/terminal: runs on next microtask.
 */
export function scheduleLayout(fn: ScheduledFn): void {
  pendingLayoutFns.push(fn);
  if (!rafScheduled) {
    rafScheduled = true;
    if (isBrowser) {
      requestAnimationFrame(flush);
    } else {
      queueMicrotask(flush);
    }
  }
}

/**
 * Force-flush all pending layout computations synchronously.
 * Useful for testing.
 */
export function flushLayout(): void {
  flush();
}

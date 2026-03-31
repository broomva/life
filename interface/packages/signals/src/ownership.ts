type CleanupFn = () => void;

interface Owner {
  parent: Owner | null;
  children: Owner[];
  cleanups: CleanupFn[];
}

let currentOwner: Owner | null = null;

export function getOwner(): Owner | null {
  return currentOwner;
}

export function runWithOwner<T>(owner: Owner, fn: () => T): T {
  const prev = currentOwner;
  currentOwner = owner;
  try {
    return fn();
  } finally {
    currentOwner = prev;
  }
}

export function createRoot<T>(fn: (dispose: () => void) => T): T {
  const owner: Owner = {
    parent: currentOwner,
    children: [],
    cleanups: [],
  };

  if (currentOwner) {
    currentOwner.children.push(owner);
  }

  function dispose() {
    disposeOwner(owner);
    if (owner.parent) {
      const idx = owner.parent.children.indexOf(owner);
      if (idx !== -1) owner.parent.children.splice(idx, 1);
    }
  }

  return runWithOwner(owner, () => fn(dispose));
}

export function onCleanup(fn: CleanupFn): void {
  if (currentOwner) {
    currentOwner.cleanups.push(fn);
  }
}

function disposeOwner(owner: Owner): void {
  // Dispose children first (depth-first)
  for (const child of owner.children) {
    disposeOwner(child);
  }
  owner.children.length = 0;

  // Then run cleanups in reverse order
  for (let i = owner.cleanups.length - 1; i >= 0; i--) {
    owner.cleanups[i]();
  }
  owner.cleanups.length = 0;
}

export type { Owner };

import type { SolvedLayout, SolvedNode } from '@life/ikr-ir';
import { effect } from '@life/ikr-signals';
import type { Signal } from '@life/ikr-signals';

/**
 * Create a reactive renderer that efficiently patches the DOM
 * when the solved layout signal changes.
 */
export function createReactiveRenderer(container: HTMLElement) {
  let prevNodes = new Map<string, HTMLElement>();

  return function update(solved: SolvedLayout): void {
    container.style.position = 'relative';
    container.style.width = `${solved.width}px`;
    container.style.height = `${solved.height}px`;

    const nextNodes = new Map<string, HTMLElement>();
    const usedIds = new Set<string>();

    for (const node of solved.nodes) {
      usedIds.add(node.id);
      const existing = prevNodes.get(node.id);

      if (existing) {
        // Patch existing element
        patchElement(existing, node);
        nextNodes.set(node.id, existing);
      } else {
        // Create new element
        const el = createElement(node);
        container.appendChild(el);
        nextNodes.set(node.id, el);
      }
    }

    // Remove elements that are no longer in the layout
    for (const [id, el] of prevNodes) {
      if (!usedIds.has(id)) {
        el.remove();
      }
    }

    prevNodes = nextNodes;
  };
}

function createElement(node: SolvedNode): HTMLElement {
  const el = document.createElement('div');
  el.dataset.ikrId = node.id;
  applyStyles(el, node);
  if (node.text) el.textContent = node.text;
  return el;
}

function patchElement(el: HTMLElement, node: SolvedNode): void {
  applyStyles(el, node);
  if (node.text !== undefined && el.textContent !== node.text) {
    el.textContent = node.text;
  }
  if (node.overflow) {
    el.dataset.overflow = 'true';
  } else {
    delete el.dataset.overflow;
  }
}

function applyStyles(el: HTMLElement, node: SolvedNode): void {
  el.style.position = 'absolute';
  el.style.left = `${node.x}px`;
  el.style.top = `${node.y}px`;
  el.style.width = `${node.width}px`;
  el.style.height = `${node.height}px`;
  el.style.overflow = 'hidden';
}

/**
 * Bind a reactive renderer to a layout signal.
 * Automatically re-renders when the signal changes.
 */
export function bindToSignal(
  container: HTMLElement,
  layoutSignal: Signal<SolvedLayout>,
): () => void {
  const renderer = createReactiveRenderer(container);
  const dispose = effect(() => {
    renderer(layoutSignal.value);
  });
  return dispose;
}

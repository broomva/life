import type { SolvedLayout, SolvedNode } from '@life/ikr-ir';

/**
 * Render a solved layout to a DOM container.
 * Uses absolute positioning based on computed x/y/width/height.
 */
export function renderToDOM(solved: SolvedLayout, container: HTMLElement): void {
  container.style.position = 'relative';
  container.style.width = `${solved.width}px`;
  container.style.height = `${solved.height}px`;

  // Clear all existing children safely (no innerHTML)
  while (container.firstChild) {
    container.removeChild(container.firstChild);
  }

  for (const node of solved.nodes) {
    const el = createDOMElement(node);
    container.appendChild(el);
  }
}

function createDOMElement(node: SolvedNode): HTMLElement {
  const el = document.createElement('div');
  el.dataset.ikrId = node.id;
  el.style.position = 'absolute';
  el.style.left = `${node.x}px`;
  el.style.top = `${node.y}px`;
  el.style.width = `${node.width}px`;
  el.style.height = `${node.height}px`;
  el.style.overflow = 'hidden';

  if (node.text) {
    el.textContent = node.text;
  }

  if (node.overflow) {
    el.dataset.overflow = 'true';
  }

  if (node.children) {
    for (const child of node.children) {
      el.appendChild(createDOMElement(child));
    }
  }

  return el;
}

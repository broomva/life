import { describe, it, expect, beforeEach } from 'vitest';
import type { SolvedLayout, SolvedNode } from '@life/ikr-ir';
import { renderToDOM } from '../render.js';
import { createReactiveRenderer } from '../reactive.js';

function makeNode(overrides: Partial<SolvedNode> & { id: string }): SolvedNode {
  return {
    x: 0,
    y: 0,
    width: 100,
    height: 50,
    overflow: false,
    ...overrides,
  };
}

function makeLayout(
  nodes: SolvedNode[],
  size: { width?: number; height?: number } = {},
): SolvedLayout {
  return {
    valid: true,
    width: size.width ?? 800,
    height: size.height ?? 600,
    nodes,
    violations: [],
  };
}

describe('renderToDOM', () => {
  let container: HTMLElement;

  beforeEach(() => {
    container = document.createElement('div');
  });

  it('creates elements with correct positioning', () => {
    const layout = makeLayout([
      makeNode({ id: 'a', x: 10, y: 20, width: 200, height: 100 }),
    ]);

    renderToDOM(layout, container);

    expect(container.style.position).toBe('relative');
    expect(container.style.width).toBe('800px');
    expect(container.style.height).toBe('600px');

    const el = container.children[0] as HTMLElement;
    expect(el.style.position).toBe('absolute');
    expect(el.style.left).toBe('10px');
    expect(el.style.top).toBe('20px');
    expect(el.style.width).toBe('200px');
    expect(el.style.height).toBe('100px');
  });

  it('sets data-ikr-id on elements', () => {
    const layout = makeLayout([
      makeNode({ id: 'node-1' }),
      makeNode({ id: 'node-2' }),
    ]);

    renderToDOM(layout, container);

    const el1 = container.children[0] as HTMLElement;
    const el2 = container.children[1] as HTMLElement;
    expect(el1.dataset.ikrId).toBe('node-1');
    expect(el2.dataset.ikrId).toBe('node-2');
  });

  it('renders text content', () => {
    const layout = makeLayout([
      makeNode({ id: 'txt', text: 'Hello World' }),
    ]);

    renderToDOM(layout, container);

    const el = container.children[0] as HTMLElement;
    expect(el.textContent).toBe('Hello World');
  });

  it('marks overflow nodes', () => {
    const layout = makeLayout([
      makeNode({ id: 'ok', overflow: false }),
      makeNode({ id: 'over', overflow: true }),
    ]);

    renderToDOM(layout, container);

    const ok = container.children[0] as HTMLElement;
    const over = container.children[1] as HTMLElement;
    expect(ok.dataset.overflow).toBeUndefined();
    expect(over.dataset.overflow).toBe('true');
  });

  it('clears previous children on re-render', () => {
    const layout1 = makeLayout([makeNode({ id: 'a' })]);
    renderToDOM(layout1, container);
    expect(container.children.length).toBe(1);

    const layout2 = makeLayout([makeNode({ id: 'b' }), makeNode({ id: 'c' })]);
    renderToDOM(layout2, container);
    expect(container.children.length).toBe(2);
    expect((container.children[0] as HTMLElement).dataset.ikrId).toBe('b');
  });

  it('renders nested children', () => {
    const layout = makeLayout([
      makeNode({
        id: 'parent',
        children: [
          makeNode({ id: 'child-1', x: 5, y: 5 }),
          makeNode({ id: 'child-2', x: 10, y: 10 }),
        ],
      }),
    ]);

    renderToDOM(layout, container);

    const parent = container.children[0] as HTMLElement;
    expect(parent.children.length).toBe(2);
    expect((parent.children[0] as HTMLElement).dataset.ikrId).toBe('child-1');
    expect((parent.children[1] as HTMLElement).dataset.ikrId).toBe('child-2');
  });
});

describe('createReactiveRenderer', () => {
  let container: HTMLElement;

  beforeEach(() => {
    container = document.createElement('div');
  });

  it('patches elements on update (change position)', () => {
    const renderer = createReactiveRenderer(container);

    renderer(makeLayout([makeNode({ id: 'a', x: 0, y: 0 })]));
    const el = container.querySelector('[data-ikr-id="a"]') as HTMLElement;
    expect(el).not.toBeNull();
    expect(el.style.left).toBe('0px');

    // Update position — same element should be reused
    renderer(makeLayout([makeNode({ id: 'a', x: 50, y: 75 })]));
    const elAfter = container.querySelector('[data-ikr-id="a"]') as HTMLElement;
    expect(elAfter).toBe(el); // same DOM reference
    expect(elAfter.style.left).toBe('50px');
    expect(elAfter.style.top).toBe('75px');
  });

  it('removes elements no longer in layout', () => {
    const renderer = createReactiveRenderer(container);

    renderer(
      makeLayout([
        makeNode({ id: 'a' }),
        makeNode({ id: 'b' }),
      ]),
    );
    expect(container.querySelectorAll('[data-ikr-id]').length).toBe(2);

    // Remove 'b' from layout
    renderer(makeLayout([makeNode({ id: 'a' })]));
    expect(container.querySelectorAll('[data-ikr-id]').length).toBe(1);
    expect(container.querySelector('[data-ikr-id="b"]')).toBeNull();
  });

  it('adds new elements', () => {
    const renderer = createReactiveRenderer(container);

    renderer(makeLayout([makeNode({ id: 'a' })]));
    expect(container.querySelectorAll('[data-ikr-id]').length).toBe(1);

    // Add 'b'
    renderer(
      makeLayout([
        makeNode({ id: 'a' }),
        makeNode({ id: 'b', text: 'new' }),
      ]),
    );
    expect(container.querySelectorAll('[data-ikr-id]').length).toBe(2);
    const b = container.querySelector('[data-ikr-id="b"]') as HTMLElement;
    expect(b.textContent).toBe('new');
  });

  it('patches text content', () => {
    const renderer = createReactiveRenderer(container);

    renderer(makeLayout([makeNode({ id: 'a', text: 'hello' })]));
    const el = container.querySelector('[data-ikr-id="a"]') as HTMLElement;
    expect(el.textContent).toBe('hello');

    renderer(makeLayout([makeNode({ id: 'a', text: 'world' })]));
    expect(el.textContent).toBe('world');
  });

  it('patches overflow flag', () => {
    const renderer = createReactiveRenderer(container);

    renderer(makeLayout([makeNode({ id: 'a', overflow: false })]));
    const el = container.querySelector('[data-ikr-id="a"]') as HTMLElement;
    expect(el.dataset.overflow).toBeUndefined();

    renderer(makeLayout([makeNode({ id: 'a', overflow: true })]));
    expect(el.dataset.overflow).toBe('true');

    renderer(makeLayout([makeNode({ id: 'a', overflow: false })]));
    expect(el.dataset.overflow).toBeUndefined();
  });
});

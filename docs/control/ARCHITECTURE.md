# Control-Aware Architecture

## Boundaries

- Interface boundary: parse/validate external input.
- Domain boundary: operate on internal typed models.
- Persistence boundary: serialize state transitions.

## Ownership

- Product modules own product behavior.
- Control modules own governance and reliability behavior.

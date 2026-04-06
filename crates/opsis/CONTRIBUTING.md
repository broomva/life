# Contributing to Opsis

Thanks for your interest in contributing to Opsis! This guide will help you get started.

## Development Setup

1. **Fork and clone** the repository
2. **Install dependencies**: `bun install`
3. **Set up environment**: Copy `apps/web/.env.example` to `apps/web/.env.local` and fill in API keys
4. **Start development**: `bun run dev`

## Code Style

- **Linter/formatter**: [Biome](https://biomejs.dev) — run `bun run lint` before committing
- **TypeScript**: Strict mode enabled in all packages
- **Commits**: Use [Conventional Commits](https://www.conventionalcommits.org/) (`feat:`, `fix:`, `docs:`, etc.)

## Pull Requests

1. Create a feature branch from `main`
2. Make your changes with clear, focused commits
3. Ensure `bun run lint` and `bun run check-types` pass
4. Open a PR with a clear description of what changed and why

## Reporting Issues

Use [GitHub Issues](https://github.com/broomva/opsis/issues) with:
- A clear title and description
- Steps to reproduce (for bugs)
- Expected vs actual behavior
- Browser/OS information if relevant

## Architecture

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for system design details.

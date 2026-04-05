.PHONY: check test lint fmt clippy regression lifecycle e2e ci

# Quick compile check
check:
	cargo check --workspace

# Run all tests (unit + integration + regression)
test:
	cargo test --workspace

# Format check
fmt:
	cargo fmt --all -- --check

# Lint
clippy:
	cargo clippy --workspace --all-targets -- -D warnings

# Lint + format
lint: fmt clippy

# Regression tests only
regression:
	cargo test -p anima-identity --test regression

# Lifecycle integration test (with output)
lifecycle:
	cargo test -p anima-identity --test lifecycle -- --nocapture

# E2E test against Arcan
e2e:
	./scripts/e2e-arcan.sh

# Full CI pipeline
ci: lint test regression lifecycle
	@echo "Anima CI: all checks passed"

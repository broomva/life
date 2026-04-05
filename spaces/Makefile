.PHONY: smoke test lint typecheck check ci control-audit recover hooks-install

# Harness targets (customized for Rust/SpacetimeDB WASM)
smoke:
	@./scripts/harness/smoke.sh

test:
	@./scripts/harness/test.sh

lint:
	@./scripts/harness/lint.sh

typecheck:
	@./scripts/harness/typecheck.sh

# Control targets
check:
	@./scripts/control/check.sh

control-audit:
	@./scripts/audit_control.sh .

recover:
	@if [ -x ./scripts/control/recover.sh ]; then ./scripts/control/recover.sh; else echo "recover primitive not installed"; exit 2; fi

hooks-install:
	@if [ -x ./scripts/control/install_hooks.sh ]; then ./scripts/control/install_hooks.sh; else echo "hooks primitive not installed"; exit 2; fi

# Composite targets
ci: smoke check test

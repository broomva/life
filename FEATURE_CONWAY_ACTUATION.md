# Planned Feature: Economic Actuation Plane (Conway-Compatible)

Status: Proposed  
Owner: Arcan + Lago + aiOS contract track  
Last updated: 2026-02-21

---

## 1) Why this feature exists

Conway highlights a capability gap common in agent systems: agents can reason, but external paid actions (compute, cloud infra, domains) still depend on fragmented human setup.

For Agent OS, the goal is **not** to hard-wire one vendor, but to add a core capability class:

- machine-payable actions,
- governed external provisioning,
- replayable/auditable economic side effects.

This feature introduces a provider-agnostic **Economic Actuation Plane** and uses Conway as the first concrete adapter.

---

## 2) Key Conway ideas worth integrating

1. Agent-native economic identity (wallet-backed identity)
2. Machine-to-machine payment flow for paid resources
3. Unified external actuation surface (cloud/compute/domains)
4. Programmatic lifecycle management (create/update/release)
5. MCP-compatible tooling for broad runtime compatibility

---

## 3) Mapping to existing aiOS primitives

| Conway Idea | aiOS Primitive | Integration Direction |
|---|---|---|
| Agent identity with economic permissions | `Capability`, `PolicyEvaluated`, provenance fields | Add economic capabilities + signer boundary; never expose raw key to model context |
| Payment flow (quote -> pay -> settle) | `EventEnvelope`, replay invariants | Add canonical payment event family (quote/authorized/executed/failed) |
| Provision infra/domain resources | Tool lifecycle + state patching | Add resource lease events + branch/session-scoped ownership |
| Autonomous operation with risk bounds | `BudgetState`, `OperatingMode`, `StateEstimated`, `GatesUpdated` | Add spend-aware homeostasis and mode downgrades on risk |
| External provider integration | Harness tool abstraction + policy middleware | Add provider-agnostic actuator port with Conway adapter implementation |

---

## 4) Contract additions (proposed)

### 4.1 Event family (canonical)

Add new event kinds:

- `PaymentQuoteReceived`
- `PaymentAuthorized`
- `PaymentExecuted`
- `PaymentFailed`
- `ResourceProvisionRequested`
- `ResourceProvisioned`
- `ResourceUpdated`
- `ResourceReleased`
- `LeaseExpired`

### 4.2 Required economic metadata

For all payment/provisioning events, require metadata fields (or typed payload fields):

- `provider` (`conway` initially)
- `quote_id`
- `tx_hash` (if paid)
- `network`, `asset`, `amount_usd`
- `resource_type`, `resource_id`
- `lease_ttl`, `owner_scope` (session/user/agent/org)
- `approval_id` (if human approval was required)

### 4.3 New invariants

1. No paid side effect without prior policy evaluation.
2. No resource provisioning without ownership + TTL.
3. Every payment must be trace-linked to intent + tool request.
4. Replay must reconstruct economic state and live-lease set deterministically.

---

## 5) Runtime and harness design

### 5.1 New actuator boundary

Introduce a provider-agnostic runtime port (name TBD):

- `quote(action)`
- `authorize(action, policy_context)`
- `execute(action)`
- `status(resource_id)`
- `release(resource_id)`

Conway becomes one adapter under this boundary.

### 5.2 Tool exposure model

Do not expose raw unconstrained provider calls. Expose bounded tools with strict schemas:

- `external.sandbox.*`
- `external.compute.*`
- `external.domains.*`
- `external.credits.*`

Default annotations for payment-bearing calls:

- `destructive=true`
- `requires_confirmation=true` (until policy thresholds allow auto)
- `open_world=true`

### 5.3 Key handling

- Keep signer isolated from model-visible context.
- Avoid surfacing key file paths in prompts/events.
- Treat signing as privileged harness operation behind policy.

---

## 6) Policy and budget model

### 6.1 Capabilities

Add capability classes:

- `payments.read`
- `payments.execute`
- `cloud.provision`
- `cloud.expose`
- `domains.register`
- `domains.dns.write`
- `compute.invoke_paid`

### 6.2 Budget controls

Extend `BudgetState` with:

- `spend_usd_today`
- `spend_usd_week`
- `spend_limit_day`
- `spend_limit_tx`
- `live_resource_count`
- `live_resource_limit`

### 6.3 Approval thresholds

Require approval for:

- First paid action in a session
- Any spend above `spend_limit_tx`
- Domain registration/renewal
- Public port exposure beyond allowed TTL

---

## 7) Homeostasis implications (Autonomic track)

Add spend/risk into state estimation:

- If spend velocity is high -> downgrade mode (`Execute` -> `Verify` or `AskHuman`)
- If live resources exceed threshold -> trigger cleanup goals
- If payment failures spike -> trip circuit breaker for paid tools

---

## 8) Rollout plan

### Phase A — Read-only discovery

- list/search/quote/credits-only tools
- no paid writes

### Phase B — Ephemeral infra

- create/delete sandbox with mandatory TTL
- no domains yet

### Phase C — Paid inference + selective provisioning

- controlled compute spend
- strict per-session/per-day budgets

### Phase D — Domain lifecycle

- register/renew/manage DNS with hard approval and audit requirements

### Phase E — Autonomous cleanup and optimization

- lease sweeper, budget-aware planning, cost/perf heuristics

---

## 9) Harness acceptance checks (must pass before broad enablement)

- `make smoke`: schema + command surface checks include actuator tools
- `make check`: policy gate tests for spend/resource thresholds
- `make test`: replay tests for payment/resource event reconstruction
- `make ci`: all above + integration tests against adapter mock/fake backend
- `make audit`: verify no payment-bearing tool lacks capability + approval path

---

## 10) Non-goals (for this track)

- Vendor lock-in to Conway-specific semantics in the core contract
- Unbounded autonomous spend
- Bypassing human approvals for high-risk economic actions
- Treating wallet/key material as ordinary tool input/output

---

## 11) Open questions

1. Canonical payload location: strongly typed fields vs metadata map?
2. Lease ownership defaults: session vs agent scope for cloud resources?
3. How strict should auto-cleanup be for long-running experiments?
4. Which events are required for “economic replay conformance” v1?

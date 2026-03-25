"""Tests for core types and payment policy."""

from haima.types import (
    ChainId,
    CreditScore,
    CreditTier,
    PaymentDecision,
    PaymentPolicy,
    USDC_CONTRACTS,
    USDC_TO_MICRO_CREDITS,
    FacilitateResponse,
    FacilitationStatus,
)


def test_chain_ids():
    assert ChainId.BASE == "eip155:8453"
    assert ChainId.BASE_SEPOLIA == "eip155:84532"
    assert ChainId.ETHEREUM == "eip155:1"


def test_usdc_contracts():
    assert USDC_CONTRACTS[ChainId.BASE].startswith("0x")
    assert len(USDC_CONTRACTS) == 3


def test_micro_credits_conversion():
    assert USDC_TO_MICRO_CREDITS == 1_000_000


def test_policy_auto_approve():
    policy = PaymentPolicy()
    assert policy.evaluate(50) == PaymentDecision.APPROVED
    assert policy.evaluate(100) == PaymentDecision.APPROVED


def test_policy_requires_approval():
    policy = PaymentPolicy()
    assert policy.evaluate(101) == PaymentDecision.REQUIRES_APPROVAL
    assert policy.evaluate(999_999) == PaymentDecision.REQUIRES_APPROVAL


def test_policy_denied():
    policy = PaymentPolicy()
    assert policy.evaluate(1_000_001) == PaymentDecision.DENIED


def test_policy_disabled():
    policy = PaymentPolicy(enabled=False)
    assert policy.evaluate(1) == PaymentDecision.DENIED


def test_policy_custom_caps():
    policy = PaymentPolicy(auto_approve_cap=500, hard_cap_per_tx=5000)
    assert policy.evaluate(500) == PaymentDecision.APPROVED
    assert policy.evaluate(501) == PaymentDecision.REQUIRES_APPROVAL
    assert policy.evaluate(5001) == PaymentDecision.DENIED


def test_facilitate_response_settled():
    resp = FacilitateResponse(
        status=FacilitationStatus.SETTLED,
        facilitator_fee_bps=15,
    )
    assert resp.status == FacilitationStatus.SETTLED
    assert resp.receipt is None


def test_facilitate_response_rejected():
    resp = FacilitateResponse(
        status=FacilitationStatus.REJECTED,
        reason="insufficient credit",
    )
    assert resp.status == FacilitationStatus.REJECTED
    assert resp.reason == "insufficient credit"


def test_policy_default_economic_mode_fields():
    """Verify allow_in_hibernate and allow_in_hustle match Rust defaults."""
    policy = PaymentPolicy()
    assert policy.allow_in_hibernate is False
    assert policy.allow_in_hustle is True


def test_credit_tier_values():
    """Credit tier enum values match Rust CreditTier serde."""
    assert CreditTier.NONE == "none"
    assert CreditTier.MICRO == "micro"
    assert CreditTier.STANDARD == "standard"
    assert CreditTier.PREMIUM == "premium"


def test_credit_tier_spending_limits():
    """Spending limits match Rust CreditTier::spending_limit()."""
    assert CreditTier.NONE.spending_limit == 0
    assert CreditTier.MICRO.spending_limit == 1_000
    assert CreditTier.STANDARD.spending_limit == 100_000
    assert CreditTier.PREMIUM.spending_limit == 10_000_000


def test_credit_score_construction():
    """CreditScore matches Rust haima-core CreditScore fields."""
    score = CreditScore(
        agent_id="agent-test",
        score=0.65,
        tier=CreditTier.STANDARD,
        spending_limit_micro_usd=100_000,
        current_balance_micro_usd=0,
    )
    assert score.agent_id == "agent-test"
    assert score.score == 0.65
    assert score.tier == CreditTier.STANDARD
    assert score.spending_limit_micro_usd == 100_000


def test_credit_score_from_json():
    """CreditScore can be deserialized from Rust-compatible JSON."""
    data = {
        "agent_id": "agent-1",
        "score": 0.85,
        "tier": "premium",
        "spending_limit_micro_usd": 10_000_000,
        "current_balance_micro_usd": -5000,
    }
    score = CreditScore(**data)
    assert score.tier == CreditTier.PREMIUM
    assert score.current_balance_micro_usd == -5000

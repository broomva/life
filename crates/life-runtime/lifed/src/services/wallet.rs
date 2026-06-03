//! life.v1.Wallet — public-plane wallet namespace.
//!
//! Each handler is ≤20 LOC. `Debit` is idempotent — replays the cached
//! response when the same `(user, project, idempotency-key, "Wallet.Debit")`
//! tuple is seen within the TTL.
//!
//! ## Pool bracketing — Sub-phase E
//!
//! Sub-phase E pushes pool bracketing inside each proxy crate's
//! `Pooled<C>` adapter (Spec C₂ §7). Wallet handlers no longer carry a
//! `pools` field — every `self.haima.<rpc>()` call brackets internally.
//! The haima breaker tracks every wallet round-trip uniformly with
//! arcan/lago/anima.

use std::sync::Arc;
use std::time::SystemTime;

use prost::Message;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};

use haima_proxy::HaimaCall;
use life_runtime_proto::life::v1 as pb;

use crate::auth::capability::CapabilityClaims;
use crate::idempotency::{IdemKey, IdempotencyStore};

pub struct WalletService {
    pub haima: Arc<dyn HaimaCall>,
    pub idem: Arc<dyn IdempotencyStore>,
}

impl WalletService {
    pub fn new(haima: Arc<dyn HaimaCall>, idem: Arc<dyn IdempotencyStore>) -> Self {
        Self { haima, idem }
    }

    fn claims<T>(req: &Request<T>) -> Result<&CapabilityClaims, Status> {
        req.extensions()
            .get::<CapabilityClaims>()
            .ok_or_else(|| Status::unauthenticated("missing capability claims"))
    }

    fn idem_key<T>(req: &Request<T>, claims: &CapabilityClaims, method: &str) -> Option<IdemKey> {
        req.metadata()
            .get("idempotency-key")
            .and_then(|v| v.to_str().ok())
            .map(|k| IdemKey {
                user_id: claims.user_id.clone(),
                project_id: claims.project_id.clone(),
                key: k.to_string(),
                method: method.to_string(),
            })
    }
}

#[tonic::async_trait]
impl pb::wallet_server::Wallet for WalletService {
    type StatementStream =
        std::pin::Pin<Box<dyn futures::Stream<Item = Result<pb::LedgerEntry, Status>> + Send>>;

    async fn get_balance(
        &self,
        req: Request<pb::WalletRef>,
    ) -> Result<Response<pb::Balance>, Status> {
        let claims_user = Self::claims(&req)?.user_id.clone();
        let r = req.get_ref();
        // BRO-1368: bind the queried wallet to the authenticated subject —
        // a capability for user A cannot read user B's balance. project_id
        // is not pinned (it selects the caller's own project wallet).
        if r.user_id != claims_user {
            return Err(Status::permission_denied(
                "get_balance: request user_id must match the capability subject",
            ));
        }
        let bal = self
            .haima
            .get_balance(&r.user_id, &r.project_id)
            .await
            .map_err(Status::from)?;
        Ok(Response::new(pb::Balance {
            micros: bal.micros,
            currency: bal.currency,
            as_of: Some(prost_types::Timestamp::from(SystemTime::now())),
        }))
    }

    async fn statement(
        &self,
        req: Request<pb::StatementReq>,
    ) -> Result<Response<Self::StatementStream>, Status> {
        let claims_user = Self::claims(&req)?.user_id.clone();
        let r = req.get_ref();
        let wref = r
            .wallet
            .as_ref()
            .ok_or_else(|| Status::invalid_argument("wallet"))?;
        // BRO-1368: bind the statement's wallet to the authenticated subject.
        if wref.user_id != claims_user {
            return Err(Status::permission_denied(
                "statement: request user_id must match the capability subject",
            ));
        }
        let since_ms = r.since.as_ref().map(|t| t.seconds * 1000).unwrap_or(0);
        let until_ms = r
            .until
            .as_ref()
            .map(|t| t.seconds * 1000)
            .unwrap_or(i64::MAX);
        let mut up = self
            .haima
            .statement(&wref.user_id, &wref.project_id, since_ms, until_ms, r.limit)
            .await
            .map_err(Status::from)?;
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        tokio::spawn(async move {
            use futures::StreamExt;
            while let Some(e) = up.next().await {
                let _ = tx
                    .send(e.map(|le| pb::LedgerEntry {
                        entry_id: le.entry_id,
                        at: Some(prost_types::Timestamp {
                            seconds: le.at_unix_ms / 1000,
                            nanos: 0,
                        }),
                        delta_micros: le.delta_micros,
                        reason: le.reason,
                        sid: le.sid,
                        skill: String::new(),
                        model: String::new(),
                        tool: String::new(),
                    }))
                    .await;
            }
        });
        Ok(Response::new(Box::pin(ReceiverStream::new(rx))))
    }

    async fn debit(
        &self,
        req: Request<pb::DebitReq>,
    ) -> Result<Response<pb::DebitReceipt>, Status> {
        let claims = Self::claims(&req)?.clone();
        let key = Self::idem_key(&req, &claims, "Wallet.Debit")
            .ok_or_else(|| Status::failed_precondition("missing idempotency-key"))?;
        if let Some(prev) = self.idem.lookup(&key).await? {
            return Ok(Response::new(
                pb::DebitReceipt::decode(&prev[..])
                    .map_err(|e| Status::internal(format!("decode: {e}")))?,
            ));
        }
        let body = req.into_inner();
        let wref = body
            .wallet
            .ok_or_else(|| Status::invalid_argument("wallet"))?;
        // BRO-1368: bind the debited wallet to the authenticated subject —
        // a capability for user A cannot debit user B's wallet.
        if wref.user_id != claims.user_id {
            return Err(Status::permission_denied(
                "debit: request user_id must match the capability subject",
            ));
        }
        let (entry_id, bal) = self
            .haima
            .debit(
                &wref.user_id,
                &wref.project_id,
                body.amount_micros,
                &body.sid,
                &body.reason,
            )
            .await
            .map_err(Status::from)?;
        let receipt = pb::DebitReceipt {
            entry_id,
            new_balance: Some(pb::Balance {
                micros: bal.micros,
                currency: bal.currency,
                as_of: Some(prost_types::Timestamp::from(SystemTime::now())),
            }),
        };
        let mut buf = Vec::with_capacity(receipt.encoded_len());
        receipt
            .encode(&mut buf)
            .map_err(|e| Status::internal(format!("encode: {e}")))?;
        self.idem.persist(key, buf).await?;
        Ok(Response::new(receipt))
    }

    async fn transfer(
        &self,
        req: Request<pb::TransferReq>,
    ) -> Result<Response<pb::TransferReceipt>, Status> {
        let claims = Self::claims(&req)?.clone();
        // Sub-phase D8 follow-up #3: Wallet.Transfer is now idempotent —
        // same envelope as Debit. A retry with the same idempotency-key
        // returns the cached TransferReceipt instead of double-transferring.
        let key = Self::idem_key(&req, &claims, "Wallet.Transfer")
            .ok_or_else(|| Status::failed_precondition("missing idempotency-key"))?;
        if let Some(prev) = self.idem.lookup(&key).await? {
            return Ok(Response::new(
                pb::TransferReceipt::decode(&prev[..])
                    .map_err(|e| Status::internal(format!("decode: {e}")))?,
            ));
        }
        let body = req.into_inner();
        let from = body.from.ok_or_else(|| Status::invalid_argument("from"))?;
        let to = body.to.ok_or_else(|| Status::invalid_argument("to"))?;
        // BRO-1368: bind the `from` (payer) wallet to the authenticated
        // subject — a capability for user A cannot transfer FROM user B's
        // wallet. `to` (the recipient) is intentionally unconstrained.
        if from.user_id != claims.user_id {
            return Err(Status::permission_denied(
                "transfer: `from` user_id must match the capability subject",
            ));
        }
        let (entry_id, fbal, tbal) = self
            .haima
            .transfer(
                &from.user_id,
                &from.project_id,
                &to.user_id,
                &to.project_id,
                body.amount_micros,
                &body.memo,
            )
            .await
            .map_err(Status::from)?;
        let receipt = pb::TransferReceipt {
            entry_id,
            from_balance: Some(pb::Balance {
                micros: fbal.micros,
                currency: fbal.currency,
                as_of: Some(prost_types::Timestamp::from(SystemTime::now())),
            }),
            to_balance: Some(pb::Balance {
                micros: tbal.micros,
                currency: tbal.currency,
                as_of: Some(prost_types::Timestamp::from(SystemTime::now())),
            }),
        };
        let mut buf = Vec::with_capacity(receipt.encoded_len());
        receipt
            .encode(&mut buf)
            .map_err(|e| Status::internal(format!("encode: {e}")))?;
        self.idem.persist(key, buf).await?;
        Ok(Response::new(receipt))
    }

    /// Initiate an x402 payment from the user's Anima-custodied wallet
    /// (BRO-1354). Forwards to haima-proxy's `x402_pay`, which dials
    /// haimad's `WalletSubstrate.X402Pay`. The substrate owns the full
    /// client round-trip + signing; this handler is a thin marshaller.
    ///
    /// Not idempotent-cached: a payment that settled on-chain cannot be
    /// safely replayed from a cache, and the substrate's per-call nonce
    /// (EIP-3009) already makes a re-submit a distinct authorization.
    /// base-sepolia only in P1 — mainnet is rejected substrate-side.
    async fn x402_pay(
        &self,
        req: Request<pb::X402PayReq>,
    ) -> Result<Response<pb::X402PayResp>, Status> {
        // BRO-1354 hardening (P20 cross-review MEDIUM): X402Pay initiates
        // an EXTERNAL payment (scope `x402:pay`, strictly more powerful
        // than an internal ledger debit), so bind the payer to the
        // authenticated identity — a capability for user A must not pay
        // from user B's wallet, even on the direct gRPC path. (The
        // bespoke lifegw HTTP route already sources the user from the
        // verified Tier-1 token.) `project_id` is intentionally NOT
        // pinned: it selects which of the caller's OWN project wallets
        // to draw from, not a cross-tenant boundary.
        let claims_user = Self::claims(&req)?.user_id.clone();
        let r = req.into_inner();
        if r.user_id != claims_user {
            return Err(Status::permission_denied(
                "x402_pay: request user_id must match the capability subject",
            ));
        }
        let outcome = self
            .haima
            .x402_pay(
                &r.user_id,
                &r.project_id,
                &r.resource_url,
                &r.network,
                r.max_amount_micros,
            )
            .await
            .map_err(Status::from)?;
        Ok(Response::new(pb::X402PayResp {
            status: outcome.status,
            tx_hash: outcome.tx_hash,
            network: outcome.network,
            recipient: outcome.recipient,
            micro_credits: outcome.micro_credits,
            declined_reason: outcome.declined_reason,
            settled: outcome.settled,
            resource_body: outcome.resource_body,
            resource_status: outcome.resource_status,
        }))
    }
}

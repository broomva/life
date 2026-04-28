//! life.v1.Wallet — public-plane wallet namespace.
//!
//! Each handler is ≤20 LOC. `Debit` is idempotent — replays the cached
//! response when the same `(user, project, idempotency-key, "Wallet.Debit")`
//! tuple is seen within the TTL.

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
    /// Sub-phase D: per-substrate pools. Every haima dispatch brackets
    /// `pools.haima.load().acquire().await?` so the haima circuit
    /// breaker + bounded semaphore enforce backpressure.
    pub pools: Arc<crate::routing::pools::SubstratePools>,
}

impl WalletService {
    pub fn new(
        haima: Arc<dyn HaimaCall>,
        idem: Arc<dyn IdempotencyStore>,
        pools: Arc<crate::routing::pools::SubstratePools>,
    ) -> Self {
        Self { haima, idem, pools }
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
        let _claims = Self::claims(&req)?;
        let r = req.get_ref();
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
        let _claims = Self::claims(&req)?;
        let r = req.get_ref();
        let wref = r
            .wallet
            .as_ref()
            .ok_or_else(|| Status::invalid_argument("wallet"))?;
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
}

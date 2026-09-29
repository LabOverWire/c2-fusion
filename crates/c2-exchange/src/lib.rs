use std::collections::HashMap;

use c2_model::C2Message;
use stitch_p2p::Store;

/// Entity name under which C2 messages live in the peer-to-peer store.
pub const ENTITY: &str = "c2msg";

#[derive(Debug, thiserror::Error)]
pub enum ExchangeError {
    #[error("serialization failed: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("store error: {0}")]
    Store(#[from] stitch_p2p::StoreError),
}

/// Publish a C2 message into the local node's converged state. It replicates to
/// peers over the sync session and is keyed by the message id, so a re-published
/// message updates in place rather than duplicating.
pub async fn ingest(store: &Store, msg: &C2Message) -> Result<(), ExchangeError> {
    let value = serde_json::to_value(msg)?;
    store.create(ENTITY, msg.id(), value).await?;
    Ok(())
}

/// The shared C2 picture as this node currently sees it: every visible message
/// keyed by id. Two converged nodes return equal pictures.
pub async fn picture(store: &Store) -> Result<HashMap<String, C2Message>, ExchangeError> {
    let mut out = HashMap::new();
    for (id, value) in store.entries(ENTITY).await {
        out.insert(id, serde_json::from_value(value)?);
    }
    Ok(out)
}

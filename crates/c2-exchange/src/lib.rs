use std::collections::HashMap;

use c2_codec::{Codec, CodecError, Registry};
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
    #[error("codec error: {0}")]
    Codec(#[from] CodecError),
}

/// Publish a C2 message into the local node's converged state. It replicates to
/// peers over the sync session and is keyed by the message id, so a re-published
/// message updates in place rather than duplicating.
pub async fn ingest(store: &Store, msg: &C2Message) -> Result<(), ExchangeError> {
    let value = serde_json::to_value(msg)?;
    store.create(ENTITY, msg.id(), value).await?;
    Ok(())
}

/// Decode a message that arrived in a wire format, then publish it. This is the
/// ingest boundary: a standards-compliant message enters, the canonical model is
/// stored and replicated. The decoded message is returned.
pub async fn ingest_wire(
    store: &Store,
    codec: &dyn Codec,
    bytes: &[u8],
) -> Result<C2Message, ExchangeError> {
    let msg = codec.decode(bytes)?;
    ingest(store, &msg).await?;
    Ok(msg)
}

/// Ingest a wire-format message, selecting the codec by wire-format name from a
/// registry. Unknown formats yield a codec error rather than a silent drop.
pub async fn ingest_wire_with(
    store: &Store,
    registry: &Registry,
    wire_format: &str,
    bytes: &[u8],
) -> Result<C2Message, ExchangeError> {
    let codec = registry
        .get(wire_format)
        .ok_or_else(|| CodecError::Unsupported(wire_format.to_string()))?;
    ingest_wire(store, codec, bytes).await
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

/// The shared C2 picture rendered out in a wire format: every visible message
/// keyed by id, encoded by `codec`. The egress boundary. A picture assembled
/// from messages that arrived in different formats can be exported in any one.
pub async fn picture_encoded(
    store: &Store,
    codec: &dyn Codec,
) -> Result<HashMap<String, Vec<u8>>, ExchangeError> {
    let mut out = HashMap::new();
    for (id, msg) in picture(store).await? {
        out.insert(id, codec.encode(&msg)?);
    }
    Ok(out)
}

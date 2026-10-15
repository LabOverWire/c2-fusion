use std::collections::HashMap;

use c2_codec::{Codec, CodecError, Registry};
use c2_model::{C2Message, InvalidMessage};
use stitch_p2p::Store;

pub mod radio;

pub const ENTITY: &str = "c2msg";

#[derive(Debug, thiserror::Error)]
pub enum ExchangeError {
    #[error("serialization failed: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("store error: {0}")]
    Store(#[from] stitch_p2p::StoreError),
    #[error("codec error: {0}")]
    Codec(#[from] CodecError),
    #[error("invalid message: {0}")]
    Invalid(#[from] InvalidMessage),
}

pub async fn ingest(store: &Store, msg: &C2Message) -> Result<(), ExchangeError> {
    msg.validate()?;
    let value = serde_json::to_value(msg)?;
    store.create(ENTITY, msg.id(), value).await?;
    Ok(())
}

pub async fn ingest_wire(
    store: &Store,
    codec: &dyn Codec,
    bytes: &[u8],
) -> Result<C2Message, ExchangeError> {
    let msg = codec.decode(bytes)?;
    ingest(store, &msg).await?;
    Ok(msg)
}

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

pub async fn picture(store: &Store) -> Result<HashMap<String, C2Message>, ExchangeError> {
    let mut out = HashMap::new();
    for (id, value) in store.entries(ENTITY).await {
        out.insert(id, serde_json::from_value(value)?);
    }
    Ok(out)
}

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

use std::collections::{BTreeSet, HashMap};

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

#[derive(Debug, Default, PartialEq)]
pub struct Picture {
    pub messages: HashMap<String, C2Message>,
    pub unreadable: BTreeSet<String>,
}

pub async fn picture(store: &Store) -> Picture {
    let mut picture = Picture::default();
    for (id, data, _) in store.node().entries_with_authors(ENTITY).await {
        match serde_json::from_slice(&data) {
            Ok(msg) => {
                picture.messages.insert(id, msg);
            }
            Err(_) => {
                picture.unreadable.insert(id);
            }
        }
    }
    picture
}

pub async fn picture_encoded(
    store: &Store,
    codec: &dyn Codec,
) -> Result<HashMap<String, Vec<u8>>, ExchangeError> {
    let mut out = HashMap::new();
    for (id, msg) in picture(store).await.messages {
        out.insert(id, codec.encode(&msg)?);
    }
    Ok(out)
}

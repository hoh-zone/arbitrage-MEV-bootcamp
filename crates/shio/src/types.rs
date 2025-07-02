use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Clone, Deserialize)]
pub enum ShioItem {
    #[serde(rename = "auctionStarted")]
    AuctionStarted {
        #[serde(rename = "txDigest")]
        tx_digest: String,
        #[serde(rename = "gasPrice")]
        gas_price: u64,
        #[serde(rename = "deadlineTimestampMs")]
        deadline_timestamp_ms: u64,
        #[serde(rename = "sideEffects")]
        side_effects: SideEffects,
        #[serde(skip)]
        _other: (),
    },

    #[serde(rename = "auctionEnded")]
    AuctionEnded {
        #[serde(rename = "txDigest")]
        tx_digest: String,
        #[serde(rename = "winningBidAmount")]
        winning_bid_amount: u64,
    },

    #[serde(skip)]
    Dummy(Value),
}

impl ShioItem {
    pub fn tx_digest(&self) -> &str {
        match self {
            ShioItem::AuctionStarted { tx_digest, .. } => tx_digest,
            ShioItem::AuctionEnded { .. } => "auctionEnded",
            ShioItem::Dummy(_) => "dummy",
        }
    }

    pub fn gas_price(&self) -> u64 {
        match self {
            ShioItem::AuctionStarted { gas_price, .. } => *gas_price,
            ShioItem::AuctionEnded { .. } => 0,
            ShioItem::Dummy(_) => 0,
        }
    }

    pub fn deadline_timestamp_ms(&self) -> u64 {
        match self {
            ShioItem::AuctionStarted {
                deadline_timestamp_ms, ..
            } => *deadline_timestamp_ms,
            ShioItem::AuctionEnded { .. } => 0,
            ShioItem::Dummy(_) => 0,
        }
    }

    pub fn events(&self) -> Vec<ShioEvent> {
        match self {
            ShioItem::AuctionStarted { side_effects, .. } => side_effects.events.clone(),
            ShioItem::AuctionEnded { .. } => vec![],
            ShioItem::Dummy(_) => vec![],
        }
    }

    pub fn created_mutated_objects(&self) -> Vec<&ShioObject> {
        match self {
            ShioItem::AuctionStarted { side_effects, .. } => side_effects
                .created_objects
                .iter()
                .chain(&side_effects.mutated_objects)
                .collect(),
            ShioItem::AuctionEnded { .. } => vec![],
            ShioItem::Dummy(_) => vec![],
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct SideEffects {
    #[serde(rename = "createdObjects", default)]
    pub created_objects: Vec<ShioObject>,
    #[serde(rename = "mutatedObjects", default)]
    pub mutated_objects: Vec<ShioObject>,
    #[serde(rename = "gasUsage")]
    pub gas_usage: u64,
    #[serde(rename = "events", default)]
    pub events: Vec<ShioEvent>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShioObject {
    pub id: String,
    pub object_type: String,
    pub owner: Value,
    pub content: ShioObjectContent,
    pub object_bcs: String, // base64 encoded
}

impl ShioObject {
    pub fn data_type(&self) -> &str {
        &self.content.data_type
    }

    pub fn has_public_transfer(&self) -> bool {
        self.content.has_public_transfer
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShioObjectContent {
    pub data_type: String,
    pub has_public_transfer: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ShioEvent {
    #[serde(rename = "type")]
    pub event_type: String,
    #[serde(rename = "bcs")]
    pub bcs: String, // base64 encoded
    #[serde(rename = "id")]
    pub event_id: ShioEventId,
    #[serde(rename = "packageId")]
    pub package_id: String,
    #[serde(rename = "parsedJson", default)]
    pub parsed_json: Option<Value>, // Could use a specific type if structure is known
    #[serde(rename = "sender")]
    pub sender: String,
    #[serde(rename = "transactionModule")]
    pub transaction_module: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ShioEventId {
    #[serde(rename = "eventSeq")]
    pub event_seq: String,
    #[serde(rename = "txDigest")]
    pub tx_digest: String,
}

impl From<Value> for ShioItem {
    fn from(value: Value) -> Self {
        serde_json::from_value(value.clone()).unwrap_or(ShioItem::Dummy(value))
    }
}

impl ShioItem {
    pub fn type_name(&self) -> &str {
        match self {
            ShioItem::AuctionStarted { .. } => "auctionStarted",
            ShioItem::AuctionEnded { .. } => "auctionEnded",
            ShioItem::Dummy(_) => "dummy",
        }
    }
}

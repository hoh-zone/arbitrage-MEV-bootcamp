use std::fmt;

use burberry::executor::telegram_message::Message;
use shio::ShioItem;
use sui_json_rpc_types::{SuiEvent, SuiTransactionBlockEffects};
use sui_types::{digests::TransactionDigest, transaction::TransactionData};

#[derive(Debug, Clone)]
pub enum Action {
    NotifyViaTelegram(Message),
    ExecutePublicTx(TransactionData),
    ShioSubmitBid((TransactionData, u64, TransactionDigest)),
}

impl From<Message> for Action {
    fn from(msg: Message) -> Self {
        Self::NotifyViaTelegram(msg)
    }
}

impl From<TransactionData> for Action {
    fn from(tx_data: TransactionData) -> Self {
        Self::ExecutePublicTx(tx_data)
    }
}

impl From<(TransactionData, u64, TransactionDigest)> for Action {
    fn from((tx_data, bid_amount, opp_tx_digest): (TransactionData, u64, TransactionDigest)) -> Self {
        Self::ShioSubmitBid((tx_data, bid_amount, opp_tx_digest))
    }
}

#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug)]
pub enum Event {
    PublicTx(SuiTransactionBlockEffects, Vec<SuiEvent>),
    PrivateTx(TransactionData),
    Shio(ShioItem),
}

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub enum Source {
    Public,
    Shio {
        opp_tx_digest: TransactionDigest,
        bid_amount: u64,
        start: u64,
        arb_found: u64,
        deadline: u64,
    },
    ShioDeadlineMissed {
        start: u64,
        arb_found: u64,
        deadline: u64,
    },
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Source::Public => write!(f, "Public"),
            Source::Shio {
                start,
                arb_found,
                deadline,
                ..
            } => write!(
                f,
                "Shio(start={}, deadline={}, time_window={}ms, arb_found={}, early={}ms)",
                *start,
                *deadline,
                (*deadline).saturating_sub(*start),
                *arb_found,
                (*deadline).saturating_sub(*arb_found)
            ),
            Source::ShioDeadlineMissed {
                start,
                arb_found,
                deadline,
            } => write!(
                f,
                "ShioDeadlineMissed(start={}, deadline={}, time_window={}ms, arb_found={}, overdue={}ms)",
                *start,
                *deadline,
                (*deadline).saturating_sub(*start),
                *arb_found,
                (*arb_found).saturating_sub(*deadline)
            ),
        }
    }
}

impl Source {
    pub fn is_shio(&self) -> bool {
        matches!(self, Source::Shio { .. })
    }

    pub fn opp_tx_digest(&self) -> Option<TransactionDigest> {
        match self {
            Source::Shio { opp_tx_digest, .. } => Some(*opp_tx_digest),
            _ => None,
        }
    }

    pub fn deadline(&self) -> Option<u64> {
        match self {
            Source::Shio { deadline, .. } => Some(*deadline),
            _ => None,
        }
    }

    pub fn bid_amount(&self) -> u64 {
        match self {
            Source::Shio { bid_amount, .. } => *bid_amount,
            _ => 0,
        }
    }

    pub fn with_bid_amount(self, bid_amount: u64) -> Self {
        match self {
            Source::Shio {
                opp_tx_digest,
                start,
                deadline,
                arb_found,
                ..
            } => Source::Shio {
                opp_tx_digest,
                bid_amount,
                start,
                deadline,
                arb_found,
            },
            _ => self,
        }
    }

    pub fn with_arb_found_time(self, arb_found: u64) -> Self {
        match self {
            Source::Shio {
                opp_tx_digest,
                start,
                deadline,
                bid_amount,
                ..
            } => {
                if arb_found < deadline {
                    Source::Shio {
                        opp_tx_digest,
                        bid_amount,
                        start,
                        arb_found,
                        deadline,
                    }
                } else {
                    Source::ShioDeadlineMissed {
                        start,
                        arb_found,
                        deadline,
                    }
                }
            }
            _ => self,
        }
    }
}

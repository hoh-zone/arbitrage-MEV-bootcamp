use std::{
    str::FromStr,
    sync::{atomic::AtomicUsize, Arc},
};

use eyre::{ensure, eyre, Result};
use shio::SHIO_GLOBAL_STATES;
use sui_sdk::SUI_COIN_TYPE;
use sui_types::{
    base_types::{ObjectID, SequenceNumber},
    transaction::{Argument, Command, ObjectArg},
    Identifier, TypeTag,
};
use tokio::sync::OnceCell;

use super::TradeCtx;

const SHIO: &str = "0x1889977f0fb56ae730e7bda8e8e32859ce78874458c74910d36121a81a615123";
static GLOBAL_STATES: OnceCell<Vec<ObjectArg>> = OnceCell::const_new();

#[derive(Clone)]
pub struct Shio {
    global_states: Vec<ObjectArg>,
    state_idx: Arc<AtomicUsize>,
}

impl Shio {
    pub async fn new() -> Result<Self> {
        let global_states = GLOBAL_STATES
            .get_or_init(|| async {
                SHIO_GLOBAL_STATES
                    .iter()
                    .map(|(id, version)| ObjectArg::SharedObject {
                        id: ObjectID::from_str(id).unwrap(),
                        initial_shared_version: SequenceNumber::from_u64(*version),
                        mutable: true,
                    })
                    .collect::<Vec<_>>()
            })
            .await
            .clone();

        let state_idx = Arc::new(AtomicUsize::new(0));

        Ok(Self {
            global_states,
            state_idx,
        })
    }

    pub fn submit_bid(&self, ctx: &mut TradeCtx, coin_bid: Argument, bid_amount: u64) -> Result<()> {
        ensure!(bid_amount > 0, "bid_amount must be greater than 0");

        let package = ObjectID::from_hex_literal(SHIO)?;
        let module = Identifier::new("auctioneer").map_err(|e| eyre!(e))?;
        let function = Identifier::new("submit_bid").map_err(|e| eyre!(e))?;

        let s = ctx.obj(self.next_state()).map_err(|e| eyre!(e))?;
        let bid_amount = ctx.pure(bid_amount).map_err(|e| eyre!(e))?;
        let coin_type = TypeTag::from_str(SUI_COIN_TYPE).unwrap();
        let fee = ctx.coin_into_balance(coin_bid, coin_type)?;
        let arguments = vec![s, bid_amount, fee];

        ctx.command(Command::move_call(package, module, function, vec![], arguments));

        Ok(())
    }

    fn next_state(&self) -> ObjectArg {
        let mut idx = self.state_idx.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if idx >= self.global_states.len() {
            idx = 0;
            self.state_idx.store(1, std::sync::atomic::Ordering::Relaxed);
        }

        self.global_states[idx]
    }
}

use std::{
    collections::HashSet,
    fmt,
    hash::{Hash, Hasher},
    sync::Arc,
};

use burberry::{async_trait, Executor};
use dashmap::DashMap;
use eyre::{bail, ensure, Result};
use serde::{Deserialize, Serialize};
use shio::ShioEvent;
use simulator::Simulator;
use sui_sdk::{
    rpc_types::{EventFilter, SuiEvent},
    types::base_types::ObjectID,
    SuiClient, SUI_COIN_TYPE,
};
use tracing::error;

use crate::{
    normalize_coin_type,
    protocols::{
        abex::*, aftermath::*, babyswap::*, blue_move::*, cetus::*, deepbook_v2::*, flowx_amm::*, flowx_clmm::*,
        interest::*, kriya_amm::*, kriya_clmm::*, navi::*, suiswap::*, turbos::*,
    },
};

// token_type -> pools
pub type TokenPools = DashMap<String, HashSet<Pool>>;
// (token0_type, token1_type) -> pools
pub type Token01Pools = DashMap<(String, String), HashSet<Pool>>;

#[derive(Debug, Clone)]
pub struct PoolCache {
    pub token_pools: Arc<TokenPools>,
    pub token01_pools: Arc<Token01Pools>,
    pub pool_map: Arc<DashMap<ObjectID, Pool>>,
}

impl PoolCache {
    pub fn new(token_pools: TokenPools, token01_pools: Token01Pools, pool_map: DashMap<ObjectID, Pool>) -> Self {
        Self {
            token_pools: Arc::new(token_pools),
            token01_pools: Arc::new(token01_pools),
            pool_map: Arc::new(pool_map),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Pool {
    pub protocol: Protocol,
    pub pool: ObjectID,
    pub tokens: Vec<Token>,
    pub extra: PoolExtra,
}

impl PartialEq for Pool {
    fn eq(&self, other: &Self) -> bool {
        self.pool == other.pool
    }
}

impl Eq for Pool {}

impl Hash for Pool {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.pool.hash(state);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Token {
    pub token_type: String,
    pub decimals: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PoolExtra {
    None,
    Cetus {
        fee_rate: u64,
    },
    Turbos {
        fee: u32,
    },
    Aftermath {
        lp_type: String,
        fees_swap_in: Vec<u64>,
        fees_swap_out: Vec<u64>,
        fees_deposit: Vec<u64>,
        fees_withdraw: Vec<u64>,
    },
    KriyaAmm {
        lp_fee_percent: u64,
        protocol_fee_percent: u64,
    },
    KriyaClmm {
        fee_rate: u64,
    },
    FlowxAmm {
        fee_rate: u64,
    },
    FlowxClmm {
        fee_rate: u64,
    },
    DeepbookV2 {
        taker_fee_rate: u64,
        maker_rebate_rate: u64,
        tick_size: u64,
        lot_size: u64,
    },
}

impl fmt::Display for Pool {
    // protocol|pool|tokens|extra
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}|{}|{}|{}",
            self.protocol,
            self.pool,
            serde_json::to_string(&self.tokens).unwrap(),
            serde_json::to_string(&self.extra).unwrap()
        )
    }
}

impl TryFrom<&str> for Pool {
    type Error = eyre::Error;

    fn try_from(value: &str) -> Result<Self> {
        let parts: Vec<&str> = value.split('|').collect();
        ensure!(parts.len() == 4, "Invalid pool format: {}", value);

        let protocol = Protocol::try_from(parts[0])?;
        let pool = parts[1].parse()?;
        let tokens: Vec<Token> = serde_json::from_str(parts[2])?;
        let extra: PoolExtra = serde_json::from_str(parts[3])?;

        Ok(Pool {
            protocol,
            pool,
            tokens,
            extra,
        })
    }
}

impl Pool {
    pub fn token0_type(&self) -> String {
        self.tokens[0].token_type.clone()
    }

    pub fn token1_type(&self) -> String {
        self.tokens[1].token_type.clone()
    }

    pub fn token_count(&self) -> usize {
        self.tokens.len()
    }

    pub fn token_index(&self, token_type: &str) -> Option<usize> {
        self.tokens.iter().position(|token| token.token_type == token_type)
    }

    pub fn token(&self, index: usize) -> Option<Token> {
        self.tokens.get(index).cloned()
    }

    // (token0_type, token1_type)
    pub fn token01_pairs(&self) -> Vec<(String, String)> {
        let mut pairs = Vec::new();
        for i in 0..self.tokens.len() {
            for j in i + 1..self.tokens.len() {
                pairs.push((self.tokens[i].token_type.clone(), self.tokens[j].token_type.clone()));
            }
        }

        pairs
    }

    pub async fn related_object_ids(&self, simulator: Arc<dyn Simulator>) -> HashSet<String> {
        let mut res = HashSet::new();

        // Pool
        res.insert(self.pool.to_string());

        // Tokens
        let token_object_ids = self
            .tokens
            .iter()
            .map(|token| token.token_type.split_once("::").unwrap().0.to_string())
            .collect::<Vec<_>>();
        res.extend(token_object_ids);

        // Children
        let children_ids = match self.protocol {
            Protocol::Cetus => cetus_pool_children_ids(self, simulator).await,
            Protocol::BlueMove => blue_move_pool_children_ids(self, simulator).await,
            Protocol::Turbos => turbos_pool_children_ids(self, simulator).await,
            Protocol::KriyaClmm => kriya_clmm_pool_children_ids(self, simulator).await,
            Protocol::FlowxClmm => flowx_clmm_pool_children_ids(self, simulator).await,
            Protocol::Aftermath => aftermath_pool_children_ids(self, simulator).await,
            _ => Ok(vec![]),
        };
        match children_ids {
            Ok(children_ids) => res.extend(children_ids),
            Err(e) => error!("Failed to get pool children ids: {}, pool: {}", e, self.pool),
        }

        res
    }
}

impl Token {
    pub fn new(token_type: &str, decimals: u8) -> Self {
        Self {
            token_type: normalize_coin_type(token_type),
            decimals,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SwapEvent {
    pub protocol: Protocol,
    pub pool: Option<ObjectID>,
    pub coins_in: Vec<String>,
    pub coins_out: Vec<String>,
    pub amounts_in: Vec<u64>,
    pub amounts_out: Vec<u64>,
}

impl SwapEvent {
    pub fn pool_id(&self) -> Option<ObjectID> {
        self.pool
    }

    pub fn involved_coin_one_side(&self) -> String {
        if self.coins_in[0] != SUI_COIN_TYPE {
            self.coins_in[0].to_string()
        } else {
            self.coins_out[0].to_string()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Protocol {
    Cetus,
    Turbos,
    Aftermath,
    KriyaAmm,
    KriyaClmm,
    FlowxAmm,
    FlowxClmm,
    DeepbookV2,
    DeepbookV3,
    Volo,
    BlueMove,
    SuiSwap,
    Interest,
    Abex,
    BabySwap,
    Navi,
}

impl fmt::Display for Protocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Protocol::Cetus => write!(f, "cetus"),
            Protocol::Turbos => write!(f, "turbos"),
            Protocol::Aftermath => write!(f, "aftermath"),
            Protocol::KriyaAmm => write!(f, "kriya_amm"),
            Protocol::KriyaClmm => write!(f, "kriya_clmm"),
            Protocol::FlowxAmm => write!(f, "flowx_amm"),
            Protocol::FlowxClmm => write!(f, "flowx_clmm"),
            Protocol::DeepbookV2 => write!(f, "deepbook_v2"),
            Protocol::DeepbookV3 => write!(f, "deepbook_v3"),
            Protocol::Volo => write!(f, "volo"),
            Protocol::BlueMove => write!(f, "blue_move"),
            Protocol::SuiSwap => write!(f, "suiswap"),
            Protocol::Interest => write!(f, "interest"),
            Protocol::Abex => write!(f, "abex"),
            Protocol::BabySwap => write!(f, "babyswap"),
            Protocol::Navi => write!(f, "navi"),
        }
    }
}

impl TryFrom<&str> for Protocol {
    type Error = eyre::Error;

    fn try_from(value: &str) -> Result<Self> {
        match value {
            "cetus" => Ok(Protocol::Cetus),
            "turbos" => Ok(Protocol::Turbos),
            "aftermath" => Ok(Protocol::Aftermath),
            "kriya_amm" => Ok(Protocol::KriyaAmm),
            "kriya_clmm" => Ok(Protocol::KriyaClmm),
            "flowx_amm" => Ok(Protocol::FlowxAmm),
            "flowx_clmm" => Ok(Protocol::FlowxClmm),
            "deepbook_v2" => Ok(Protocol::DeepbookV2),
            "deepbook_v3" => Ok(Protocol::DeepbookV3),
            "volo" => Ok(Protocol::Volo),
            "blue_move" => Ok(Protocol::BlueMove),
            "suiswap" => Ok(Protocol::SuiSwap),
            "interest" => Ok(Protocol::Interest),
            "abex" => Ok(Protocol::Abex),
            "babyswap" => Ok(Protocol::BabySwap),
            "navi" => Ok(Protocol::Navi),
            _ => bail!("Unsupported protocol: {}", value),
        }
    }
}

impl TryFrom<&SuiEvent> for Protocol {
    type Error = eyre::Error;

    fn try_from(event: &SuiEvent) -> Result<Self> {
        Self::try_from_event_type(&event.type_.to_string())
    }
}

impl TryFrom<&ShioEvent> for Protocol {
    type Error = eyre::Error;

    fn try_from(event: &ShioEvent) -> Result<Self> {
        Self::try_from_event_type(&event.event_type)
    }
}

impl Protocol {
    pub fn try_from_event_type(event_type: &str) -> Result<Self> {
        match event_type {
            CETUS_SWAP_EVENT => Ok(Protocol::Cetus),
            TURBOS_SWAP_EVENT => Ok(Protocol::Turbos),
            AFTERMATH_SWAP_EVENT => Ok(Protocol::Aftermath),
            event_type if event_type.starts_with(KRIYA_AMM_SWAP_EVENT) => Ok(Protocol::KriyaAmm),
            KRIYA_CLMM_SWAP_EVENT => Ok(Protocol::KriyaClmm),
            FLOWX_AMM_SWAP_EVENT => Ok(Protocol::FlowxAmm),
            FLOWX_CLMM_SWAP_EVENT => Ok(Protocol::FlowxClmm),
            event_type if event_type.starts_with(BLUE_MOVE_SWAP_EVENT) => Ok(Protocol::BlueMove),
            event_type if event_type.starts_with(SUISWAP_SWAP_EVENT) => Ok(Protocol::SuiSwap),
            event_type if event_type.starts_with(INTEREST_SWAP_EVENT) => Ok(Protocol::Interest),
            event_type if event_type.starts_with(ABEX_SWAP_EVENT) => Ok(Protocol::Abex),
            event_type if event_type.starts_with(BABY_SWAP_EVENT) => Ok(Protocol::BabySwap),
            _ => bail!("Not interesting"),
        }
    }

    pub fn event_filter(&self) -> EventFilter {
        match self {
            Protocol::Cetus => cetus_event_filter(),
            Protocol::Turbos => turbos_event_filter(),
            Protocol::Aftermath => aftermath_event_filter(),
            Protocol::KriyaAmm => kriya_amm_event_filter(),
            Protocol::KriyaClmm => kriya_clmm_event_filter(),
            Protocol::FlowxAmm => flowx_amm_event_filter(),
            Protocol::FlowxClmm => flowx_clmm_event_filter(),
            Protocol::DeepbookV2 => deepbook_v2_event_filter(),
            Protocol::BlueMove => blue_move_event_filter(),
            _ => todo!(),
        }
    }

    pub async fn sui_event_to_pool(&self, event: &SuiEvent, sui: &SuiClient) -> Result<Pool> {
        match self {
            Protocol::Cetus => CetusPoolCreated::try_from(event)?.to_pool(sui).await,
            Protocol::Turbos => TurbosPoolCreated::try_from(event)?.to_pool(sui).await,
            Protocol::Aftermath => AftermathPoolCreated::try_from(event)?.to_pool(sui).await,
            Protocol::KriyaAmm => KriyaAmmPoolCreated::try_from(event)?.to_pool(sui).await,
            Protocol::KriyaClmm => KriyaClmmPoolCreated::try_from(event)?.to_pool(sui).await,
            Protocol::FlowxAmm => FlowxAmmPoolCreated::try_from(event)?.to_pool(sui).await,
            Protocol::FlowxClmm => FlowxClmmPoolCreated::try_from(event)?.to_pool(sui).await,
            Protocol::DeepbookV2 => DeepbookV2PoolCreated::try_from(event)?.to_pool(sui).await,
            Protocol::BlueMove => BlueMovePoolCreated::try_from(event)?.to_pool(sui).await,
            _ => todo!(),
        }
    }

    pub async fn sui_event_to_swap_event(&self, event: &SuiEvent, provider: Arc<dyn Simulator>) -> Result<SwapEvent> {
        match self {
            Protocol::Cetus => CetusSwapEvent::try_from(event)?.to_swap_event_v2(provider).await,
            Protocol::Turbos => TurbosSwapEvent::try_from(event)?.to_swap_event_v2(provider).await,
            Protocol::Aftermath => AftermathSwapEvent::try_from(event)?.to_swap_event().await,
            Protocol::KriyaAmm => KriyaAmmSwapEvent::try_from(event)?.to_swap_event_v2(provider).await,
            Protocol::KriyaClmm => KriyaClmmSwapEvent::try_from(event)?.to_swap_event_v2(provider).await,
            Protocol::FlowxAmm => FlowxAmmSwapEvent::try_from(event)?.to_swap_event().await,
            Protocol::FlowxClmm => FlowxClmmSwapEvent::try_from(event)?.to_swap_event_v2(provider).await,
            Protocol::BlueMove => BlueMoveSwapEvent::try_from(event)?.to_swap_event().await,
            Protocol::SuiSwap => SuiswapSwapEvent::try_from(event)?.to_swap_event().await,
            Protocol::Interest => InterestSwapEvent::try_from(event)?.to_swap_event().await,
            Protocol::Abex => AbexSwapEvent::try_from(event)?.to_swap_event().await,
            Protocol::BabySwap => BabySwapEvent::try_from(event)?.to_swap_event().await,
            _ => todo!(),
        }
    }

    pub async fn shio_event_to_swap_event(&self, event: &ShioEvent, provider: Arc<dyn Simulator>) -> Result<SwapEvent> {
        match self {
            Protocol::Cetus => CetusSwapEvent::try_from(event)?.to_swap_event_v2(provider).await,
            Protocol::Turbos => TurbosSwapEvent::try_from(event)?.to_swap_event_v2(provider).await,
            Protocol::Aftermath => AftermathSwapEvent::try_from(event)?.to_swap_event().await,
            Protocol::KriyaAmm => KriyaAmmSwapEvent::try_from(event)?.to_swap_event_v2(provider).await,
            Protocol::KriyaClmm => KriyaClmmSwapEvent::try_from(event)?.to_swap_event_v2(provider).await,
            Protocol::FlowxAmm => FlowxAmmSwapEvent::try_from(event)?.to_swap_event().await,
            Protocol::FlowxClmm => FlowxClmmSwapEvent::try_from(event)?.to_swap_event_v2(provider).await,
            Protocol::BlueMove => BlueMoveSwapEvent::try_from(event)?.to_swap_event().await,
            Protocol::SuiSwap => SuiswapSwapEvent::try_from(event)?.to_swap_event().await,
            Protocol::Interest => InterestSwapEvent::try_from(event)?.to_swap_event().await,
            Protocol::Abex => AbexSwapEvent::try_from(event)?.to_swap_event().await,
            Protocol::BabySwap => BabySwapEvent::try_from(event)?.to_swap_event().await,
            _ => todo!(),
        }
    }

    pub async fn related_object_ids(&self) -> Result<HashSet<String>> {
        let res = match self {
            Protocol::Cetus => cetus_related_object_ids(),
            Protocol::BlueMove => blue_move_related_object_ids(),
            Protocol::Turbos => turbos_related_object_ids(),
            Protocol::KriyaAmm => kriya_amm_related_object_ids(),
            Protocol::KriyaClmm => kriya_clmm_related_object_ids(),
            Protocol::FlowxClmm => flowx_clmm_related_object_ids(),
            Protocol::Navi => navi_related_object_ids(),
            Protocol::Aftermath => aftermath_related_object_ids().await,
            _ => bail!("Not interesting"),
        }
        .into_iter()
        .collect::<HashSet<String>>();

        Ok(res)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    QueryEventTrigger,
}

#[derive(Debug, Clone)]
pub struct NoAction;

#[derive(Debug, Clone)]
pub struct DummyExecutor;

#[async_trait]
impl Executor<NoAction> for DummyExecutor {
    async fn execute(&self, _action: NoAction) -> Result<()> {
        Ok(())
    }

    fn name(&self) -> &'static str {
        "DummyDexIndexerExecutor"
    }
}

//! Example:
//! cargo run -r --bin arb run --coin-type \
//!     "0xa8816d3a6e3136e86bc2873b1f94a15cadc8af2703c075f2d546c2ae367f4df9::ocean::OCEAN"

use std::{
    fmt,
    str::FromStr,
    sync::Arc,
    time::{Duration, Instant},
};

use async_trait::async_trait;
use clap::Parser;
use eyre::{ensure, ContextCompat, Result};
use itertools::Itertools;
use object_pool::ObjectPool;
use simulator::{HttpSimulator, SimulateCtx, Simulator};
use sui_sdk::SuiClientBuilder;
use sui_types::{
    base_types::{ObjectID, ObjectRef, SuiAddress},
    transaction::TransactionData,
};
use tokio::task::JoinSet;
use tracing::{debug, info, instrument, Instrument};
use utils::coin;

use crate::{
    common::get_latest_epoch,
    common::search::{golden_section_search_maximize, SearchGoal},
    defi::{Defi, Path, TradeType},
    types::Source,
    HttpConfig,
};

#[derive(Clone, Debug, Parser)]
pub struct Args {
    #[arg(long)]
    pub coin_type: String,

    #[arg(long)]
    pub pool_id: Option<String>,

    #[arg(
        long,
        default_value = ""
    )]
    pub sender: String,

    #[command(flatten)]
    pub http_config: HttpConfig,
}

pub async fn run(args: Args) -> Result<()> {
    mev_logger::init_console_logger_with_directives(None, &["arb=debug", "dex_indexer=debug"]);

    info!("Running arb with {:?}", args);
    let rpc_url = args.http_config.rpc_url.clone();
    let ipc_path = args.http_config.ipc_path.clone();

    let sender = SuiAddress::from_str(&args.sender).map_err(|e| eyre::eyre!(e))?;

    let simulator_pool = ObjectPool::new(1, move || {
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(async { Box::new(HttpSimulator::new(&rpc_url, &ipc_path).await) as Box<dyn Simulator> })
    });

    let arb = Arb::new(&args.http_config.rpc_url, Arc::new(simulator_pool)).await?;
    let sui = SuiClientBuilder::default().build(&args.http_config.rpc_url).await?;
    let gas_coins = coin::get_gas_coin_refs(&sui, sender, None).await?;
    let epoch = get_latest_epoch(&sui).await?;
    let sim_ctx = SimulateCtx::new(epoch, vec![]);
    let pool_id = args.pool_id.as_deref().map(ObjectID::from_hex_literal).transpose()?;

    let result = arb
        .find_opportunity(
            sender,
            &args.coin_type,
            pool_id,
            gas_coins,
            sim_ctx,
            true,
            Source::Public,
        )
        .await?;

    info!("{result:#?}");
    Ok(())
}

#[derive(Debug)]
pub struct ArbResult {
    pub create_trial_ctx_duration: Duration,
    pub grid_search_duration: Duration,
    pub gss_duration: Option<Duration>,
    pub best_trial_result: TrialResult,
    pub cache_misses: u64,
    pub source: Source,
    pub tx_data: TransactionData,
}

pub struct Arb {
    defi: Defi,
}

impl Arb {
    pub async fn new(http_url: &str, simulator_pool: Arc<ObjectPool<Box<dyn Simulator>>>) -> Result<Self> {
        let defi = Defi::new(http_url, simulator_pool).await?;
        Ok(Self { defi })
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn find_opportunity(
        &self,
        sender: SuiAddress,
        coin_type: &str,
        pool_id: Option<ObjectID>,
        gas_coins: Vec<ObjectRef>,
        sim_ctx: SimulateCtx,
        use_gss: bool,
        source: Source,
    ) -> Result<ArbResult> {
        let gas_price = sim_ctx.epoch.gas_price;

        let (ctx, create_trial_ctx_duration) = {
            let timer = Instant::now();
            let ctx = Arc::new(
                TrialCtx::new(
                    self.defi.clone(),
                    sender,
                    coin_type,
                    pool_id,
                    gas_coins.clone(),
                    sim_ctx,
                )
                .await?,
            );

            (ctx, timer.elapsed())
        };

        // Grid search
        let starting_grid = 1_000_000u64; // 0.001 SUI
        let mut cache_misses = 0;
        let (mut max_trial_res, grid_search_duration) = {
            let timer = Instant::now();
            let mut joinset = JoinSet::new();
            for inc in 1..11 {
                let ctx = ctx.clone();
                let grid = starting_grid.checked_mul(10u64.pow(inc)).context("Grid overflow")?;

                joinset.spawn(async move { ctx.trial(grid).await }.in_current_span());
            }

            let mut max_trial_res = TrialResult::default();
            while let Some(Ok(trial_res)) = joinset.join_next().await {
                // debug!(?trial_res, "Grid searching");
                if let Ok(trial_res) = trial_res {
                    if trial_res.cache_misses > cache_misses {
                        cache_misses = trial_res.cache_misses;
                    }
                    if trial_res > max_trial_res {
                        max_trial_res = trial_res;
                    }
                }
            }
            (max_trial_res, timer.elapsed())
        };

        ensure!(
            max_trial_res.profit > 0,
            "cache_misses: {}. No profitable grid found",
            cache_misses
        );

        let gss_duration = if use_gss {
            // GSS
            let timer = Instant::now();
            let upper_bound = max_trial_res.amount_in.saturating_mul(10);
            let lower_bound = max_trial_res.amount_in.saturating_div(10);

            let goal = TrialGoal;
            let (_, _, trial_res) = golden_section_search_maximize(lower_bound, upper_bound, goal, &ctx).await;
            if trial_res.cache_misses > cache_misses {
                cache_misses = trial_res.cache_misses;
            }
            if trial_res > max_trial_res {
                max_trial_res = trial_res;
            }

            Some(timer.elapsed())
        } else {
            None
        };

        ensure!(
            max_trial_res.profit > 0,
            "cache_misses: {}. No profitable trade path found",
            cache_misses
        );

        let TrialResult {
            amount_in,
            trade_path,
            profit,
            ..
        } = &max_trial_res;

        let mut source = source;
        if source.deadline().is_some() {
            source = source.with_arb_found_time(utils::current_time_ms());
        }
        // TODO make bid_amount configurable
        source = source.with_bid_amount(*profit / 10 * 9);

        let tx_data = self
            .defi
            .build_final_tx_data(sender, *amount_in, trade_path, gas_coins, gas_price, source)
            .await?;

        Ok(ArbResult {
            create_trial_ctx_duration,
            grid_search_duration,
            gss_duration,
            best_trial_result: max_trial_res,
            cache_misses,
            source,
            tx_data,
        })
    }
}

pub struct TrialCtx {
    defi: Defi,
    sender: SuiAddress,
    coin_type: String,
    pool_id: Option<ObjectID>,
    buy_paths: Vec<Path>,
    sell_paths: Vec<Path>,
    gas_coins: Vec<ObjectRef>,
    sim_ctx: SimulateCtx,
}

impl TrialCtx {
    pub async fn new(
        defi: Defi,
        sender: SuiAddress,
        coin_type: &str,
        pool_id: Option<ObjectID>,
        gas_coins: Vec<ObjectRef>,
        sim_ctx: SimulateCtx,
    ) -> Result<Self> {
        let buy_paths = defi.find_buy_paths(coin_type).await?;
        ensure!(!buy_paths.is_empty(), "no buy paths found for {}", coin_type);

        let sell_paths = defi.find_sell_paths(coin_type).await?;
        ensure!(!sell_paths.is_empty(), "no sell paths found for {}", coin_type);

        if pool_id.is_some() {
            let buy_paths_contain_pool = buy_paths.iter().any(|p| p.contains_pool(pool_id));
            let sell_paths_contain_pool = sell_paths.iter().any(|p| p.contains_pool(pool_id));
            ensure!(
                buy_paths_contain_pool || sell_paths_contain_pool,
                "no paths found for the fluctuating pool: {:?}",
                pool_id
            );
        }

        Ok(Self {
            defi,
            sender,
            coin_type: coin_type.to_string(),
            pool_id,
            buy_paths,
            sell_paths,
            gas_coins,
            sim_ctx,
        })
    }

    #[instrument(
        name = "trial",
        skip_all,
        fields(
            in = %format!("{:<15}", (amount_in as f64 / 1_000_000_000.0)),
            len = %format!("{:<2}", self.buy_paths.len()),
            action="init"
        )
    )]
    pub async fn trial(&self, amount_in: u64) -> Result<TrialResult> {
        tracing::Span::current().record("action", "buy");

        let timer = Instant::now();
        let best_buy_res = self
            .defi
            .find_best_path_exact_in(
                &self.buy_paths,
                self.sender,
                amount_in,
                TradeType::Swap,
                &self.gas_coins,
                &self.sim_ctx,
            )
            .await?;
        let buy_elapsed = timer.elapsed();

        let timer = Instant::now();
        // append sell paths to the best buy path
        let best_buy_path = best_buy_res.path;
        let buy_path_contains_pool = best_buy_path.contains_pool(self.pool_id);
        let trade_paths = self
            .sell_paths
            .iter()
            .filter_map(|p| {
                // - buy_path and sell_path should not have common pools
                // - either buy_path or sell_path should contain the swapped_pool
                if best_buy_path.is_disjoint(p) && (buy_path_contains_pool || p.contains_pool(self.pool_id)) {
                    let mut path = best_buy_path.clone();
                    path.path.extend(p.path.clone());
                    Some(path)
                } else {
                    None
                }
            })
            .collect_vec();
        ensure!(
            !trade_paths.is_empty(),
            "no trade paths found for coin {}, pool_id: {:?}",
            self.coin_type,
            self.pool_id
        );

        tracing::Span::current().record("action", "sell");
        let best_trade_res = self
            .defi
            .find_best_path_exact_in(
                &trade_paths,
                self.sender,
                amount_in,
                TradeType::Flashloan,
                &self.gas_coins,
                &self.sim_ctx,
            )
            .await?;

        let sell_elapsed = timer.elapsed();
        debug!(coin_type = ?self.coin_type, result = %best_trade_res, ?buy_elapsed, ?sell_elapsed, "trial result");

        let profit = best_trade_res.profit();
        if profit <= 0 {
            return Ok(TrialResult::default());
        }

        let result = TrialResult::new(
            &self.coin_type,
            amount_in,
            profit as u64,
            best_trade_res.path,
            best_trade_res.cache_misses,
        );

        Ok(result)
    }
}

#[derive(Debug, Default, Clone)]
pub struct TrialResult {
    pub coin_type: String,
    pub amount_in: u64,
    pub profit: u64,
    pub trade_path: Path,
    pub cache_misses: u64,
}

impl PartialOrd for TrialResult {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.profit.partial_cmp(&other.profit)
    }
}

impl PartialEq for TrialResult {
    fn eq(&self, other: &Self) -> bool {
        self.profit == other.profit
    }
}

impl TrialResult {
    pub fn new(coin_type: &str, amount_in: u64, profit: u64, trade_path: Path, cache_misses: u64) -> Self {
        Self {
            coin_type: coin_type.to_string(),
            amount_in,
            profit,
            trade_path,
            cache_misses,
        }
    }
}

impl fmt::Display for TrialResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "TrialResult {{ coin_type: {}, amount_in: {}, profit: {}, trade_path: {:?} ... }}",
            self.coin_type, self.amount_in, self.profit, self.trade_path
        )
    }
}

pub struct TrialGoal;

#[async_trait]
impl SearchGoal<TrialCtx, u64, TrialResult> for TrialGoal {
    async fn evaluate(&self, amount_in: u64, ctx: &TrialCtx) -> (u64, TrialResult) {
        let trial_res = ctx.trial(amount_in).await.unwrap_or_default();
        (trial_res.profit, trial_res)
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use simulator::{DBSimulator, HttpSimulator, Simulator};
    use sui_types::base_types::SuiAddress;

    use super::*;
    use crate::config::tests::{TEST_ATTACKER, TEST_HTTP_URL};

    #[tokio::test]
    async fn test_find_best_trade_path() {
        mev_logger::init_console_logger_with_directives(None, &["arb=debug"]);

        let simulator_pool = ObjectPool::new(1, move || {
            tokio::runtime::Runtime::new()
                .unwrap()
                .block_on(async { Box::new(HttpSimulator::new(&TEST_HTTP_URL, &None).await) as Box<dyn Simulator> })
        });

        let start = Instant::now();

        let sender = SuiAddress::from_str(TEST_ATTACKER).unwrap();
        let sui = SuiClientBuilder::default().build(TEST_HTTP_URL).await.unwrap();
        let epoch = get_latest_epoch(&sui).await.unwrap();
        let sim_ctx = SimulateCtx::new(epoch, vec![]);

        let gas_coins = coin::get_gas_coin_refs(&sui, sender, None).await.unwrap();
        let arb = Arb::new(TEST_HTTP_URL, Arc::new(simulator_pool)).await.unwrap();
        let coin_type = "0xce7ff77a83ea0cb6fd39bd8748e2ec89a3f41e8efdc3f4eb123e0ca37b184db2::buck::BUCK";

        let arb_res = arb
            .find_opportunity(
                sender,
                coin_type,
                None,
                gas_coins,
                sim_ctx.clone(),
                true,
                Source::Public,
            )
            .await
            .unwrap();
        info!(?arb_res, "Best trade path");

        info!("Creating DB simulator ...");
        let db_sim: Arc<dyn Simulator> = Arc::new(DBSimulator::new_default_slow().await);
        info!("DB simulator created in {:?}", start.elapsed());

        let tx_data = arb_res.tx_data;
        let http_sim: Arc<dyn Simulator> = Arc::new(HttpSimulator::new(TEST_HTTP_URL, &None).await);

        let http_res = http_sim.simulate(tx_data.clone(), sim_ctx.clone()).await.unwrap();
        info!(?http_res, "🧀 HTTP simulation result");

        let db_res = db_sim.simulate(tx_data, sim_ctx).await.unwrap();
        info!(?db_res, "🧀 DB simulation result");
    }
}

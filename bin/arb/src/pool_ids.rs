use std::collections::HashSet;
use std::fs;
use std::str::FromStr;
use std::sync::Arc;

use clap::Parser;
use dex_indexer::{types::Protocol, DexIndexer};
use eyre::Result;
use mev_logger::LevelFilter;
use object_pool::ObjectPool;
use simulator::{DBSimulator, SimulateCtx, Simulator};
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use sui_sdk::types::{
    BRIDGE_PACKAGE_ID, DEEPBOOK_PACKAGE_ID, MOVE_STDLIB_PACKAGE_ID, SUI_AUTHENTICATOR_STATE_OBJECT_ID,
    SUI_BRIDGE_OBJECT_ID, SUI_CLOCK_OBJECT_ID, SUI_DENY_LIST_OBJECT_ID, SUI_FRAMEWORK_PACKAGE_ID,
    SUI_RANDOMNESS_STATE_OBJECT_ID, SUI_SYSTEM_PACKAGE_ID, SUI_SYSTEM_STATE_OBJECT_ID,
};
use sui_sdk::SuiClientBuilder;
use sui_types::base_types::{ObjectID, SuiAddress};
use sui_types::object::{Object, Owner};
use sui_types::transaction::{InputObjectKind, ObjectReadResult};
use tracing::info;

use crate::common::get_latest_epoch;
use crate::defi::{DexSearcher, IndexerDexSearcher, TradeType, Trader};
use crate::HttpConfig;

#[derive(Clone, Debug, Parser)]
pub struct Args {
    #[clap(long, default_value = "./pool_related_ids.txt")]
    pub result_path: String,

    #[command(flatten)]
    pub http_config: HttpConfig,

    #[clap(long, help = "Run test only")]
    pub test: bool,

    #[clap(long, help = "Simulate with fallback")]
    pub with_fallback: bool,

    #[clap(long, default_value = "10000000")]
    pub amount_in: u64,

    #[clap(
        long,
        default_value = "0x3c3dd05e348fba5d8bf6958369cc3b33c8e8be85c96e10b1ca6413ad1b2d7787,0xe356c686eb19972e076b6906de12354a1a7ce1b09691416e9d852b04fd21b9a6,0xade90c3bc407eaa34068129d63bba5d1cf7889a2dbaabe5eb9b3efbbf53891ea,0xda49f921560e39f15d801493becf79d47c89fb6db81e0cbbe7bf6d3318117a00"
    )]
    pub path: String,

    #[clap(long, help = "Delete objects before simulation")]
    pub delete_objects: Option<String>,
}

fn supported_protocols() -> Vec<Protocol> {
    vec![
        Protocol::Cetus,
        Protocol::Turbos,
        Protocol::KriyaAmm,
        Protocol::BlueMove,
        Protocol::KriyaClmm,
        Protocol::FlowxClmm,
        Protocol::Navi,
        Protocol::Aftermath,
    ]
}

/// Write all pool and related object ids to the `args.result_path`.
pub async fn run(args: Args) -> Result<()> {
    mev_logger::init_console_logger_with_directives(
        Some(LevelFilter::INFO),
        &[
            "arb=debug",
            // "dex_indexer=warn",
            // "simulator=trace",
            // "sui_types=trace",
            // "sui_move_natives_latest=trace",
            // "sui_execution=warn",
        ],
    );
    if args.test {
        return test_pool_related_objects(args).await;
    }

    let result_path = args.result_path;
    let rpc_url = args.http_config.rpc_url;

    let dex_indexer = DexIndexer::new(&rpc_url).await?;
    let simulator: Arc<dyn Simulator> = Arc::new(DBSimulator::new_default_slow().await);

    let _ = fs::remove_file(&result_path);
    let file = File::create(&result_path)?;
    let mut writer = BufWriter::new(file);

    // load existing ids
    let mut object_ids: HashSet<String> = fs::read_to_string(&result_path)?
        .lines()
        .map(|line| line.to_string())
        .collect();

    // add new ids
    for protocol in supported_protocols() {
        // protocol related ids
        object_ids.extend(protocol.related_object_ids().await?);
        if protocol == Protocol::Navi {
            // Navi pools are not indexed
            continue;
        }

        // pool related ids
        for pool in dex_indexer.get_all_pools(&protocol)? {
            object_ids.extend(pool.related_object_ids(simulator.clone()).await);
        }
    }

    object_ids.extend(global_ids());

    let all_ids: Vec<String> = object_ids.into_iter().collect();
    writeln!(writer, "{}", all_ids.join("\n"))?;

    writer.flush()?;

    info!("🎉 write pool and related object ids to {}", result_path);

    Ok(())
}

fn global_ids() -> HashSet<String> {
    // System IDs
    let mut result = vec![
        MOVE_STDLIB_PACKAGE_ID,
        SUI_FRAMEWORK_PACKAGE_ID,
        SUI_SYSTEM_PACKAGE_ID,
        BRIDGE_PACKAGE_ID,
        DEEPBOOK_PACKAGE_ID,
        SUI_SYSTEM_STATE_OBJECT_ID,
        SUI_CLOCK_OBJECT_ID,
        SUI_AUTHENTICATOR_STATE_OBJECT_ID,
        SUI_RANDOMNESS_STATE_OBJECT_ID,
        SUI_BRIDGE_OBJECT_ID,
        SUI_DENY_LIST_OBJECT_ID,
    ]
    .into_iter()
    .map(|id| id.to_string())
    .collect::<HashSet<String>>();

    // Add more global IDs here
    result.insert("0x5306f64e312b581766351c07af79c72fcb1cd25147157fdc2f8ad76de9a3fb6a".to_string()); // Wormhole
    result.insert("0x26efee2b51c911237888e5dc6702868abca3c7ac12c53f76ef8eba0697695e3d".to_string()); // Wormhole 1

    result
}

async fn test_pool_related_objects(args: Args) -> Result<()> {
    // Test Data ==================================
    let sender = SuiAddress::from_str("0xac5bceec1b789ff840d7d4e6ce4ce61c90d190a7f8c4f4ddf0bff6ee2413c33c").unwrap();
    let amount_in = args.amount_in;

    let path = args
        .path
        .split(',')
        .map(|obj_id| ObjectID::from_hex_literal(obj_id).unwrap())
        .collect::<Vec<_>>();

    let with_fallback = args.with_fallback;
    let rpc_url = args.http_config.rpc_url;

    let simulator_pool = Arc::new(ObjectPool::new(1, move || {
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(async { Box::new(DBSimulator::new_test(with_fallback).await) as Box<dyn Simulator> })
    }));

    let dex_searcher: Arc<dyn DexSearcher> = Arc::new(IndexerDexSearcher::new(&rpc_url, simulator_pool.clone()).await?);
    let path = dex_searcher.find_test_path(&path).await?;
    info!(?with_fallback, ?amount_in, ?path, ?args.delete_objects, "test data");
    // Test Data ==================================

    let sui = SuiClientBuilder::default().build(&rpc_url).await?;
    let epoch = get_latest_epoch(&sui).await?;

    // Get all pool-related objects;
    let mut override_objects = pool_related_objects(&args.result_path).await?;
    if let Some(delete_objects) = args.delete_objects {
        let delete_objects = delete_objects
            .split(',')
            .map(|obj_id| ObjectID::from_hex_literal(obj_id).unwrap())
            .collect::<Vec<_>>();
        override_objects.retain(|obj| !delete_objects.contains(&obj.id()));
    }

    let sim_ctx = SimulateCtx::new(epoch, override_objects);

    let trader = Trader::new(simulator_pool).await?;
    let result = trader
        .get_trade_result(&path, sender, amount_in, TradeType::Flashloan, vec![], sim_ctx)
        .await?;
    info!(?result, "trade result");

    Ok(())
}

async fn pool_related_objects(file_path: &str) -> Result<Vec<ObjectReadResult>> {
    let simulator: Arc<dyn Simulator> = Arc::new(DBSimulator::new_test(true).await);
    let file = File::open(file_path)?;
    let reader = BufReader::new(file);

    let mut res = vec![];
    for line in reader.lines() {
        let line = line?;
        let object_id = ObjectID::from_hex_literal(&line)?;
        let object: Object = if let Some(obj) = simulator.get_object(&object_id).await {
            obj
        } else {
            continue;
        };

        let input_object_kind = match object.owner() {
            Owner::Shared { initial_shared_version } => InputObjectKind::SharedMoveObject {
                id: object_id,
                initial_shared_version: *initial_shared_version,
                mutable: true,
            },
            _ => InputObjectKind::ImmOrOwnedMoveObject(object.compute_object_reference()),
        };

        res.push(ObjectReadResult::new(input_object_kind, object.into()));
    }

    Ok(res)
}

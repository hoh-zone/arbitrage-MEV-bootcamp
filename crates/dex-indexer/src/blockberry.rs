//! Doc: https://docs.blockberry.one/reference/getcoinbycointype

use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

use eyre::{ensure, eyre, Result};
use lazy_static::lazy_static;
use tracing::info;

const API_URL: &str = "https://api.blockberry.one/sui/v1/coins";
const BLOCKBERRY_API_KEYS: [&str; 15] = [
    "SeU8FUVFNWl825Lpneb9D5fFv7gZF0",
    "ozugPfC7PXcHEPcFAkU25185OJxXfD",
    "GmTG2rfzSy3l3LUuofGsKgc8xghypq",
    "SYCDHKBbKLrq02qCbJY2ChE1HumnjF",
    "r3Rtd6FpDGdIzSd3z2PClebgXuo2Z9",
    "9HNGXdgITDnOm1N3wjJAlr63WqrS0W",
    "XunW1WLj41GIJlkincCVew8lkrLr4K",
    "a3jn7Hca8JTd6gaIzvguWrsjrbmINc",
    "iR4QMxVHyGceaMz23zjr3JD4rBJDgc",
    "KFn5y90CJyA3I7yh5RYvukzzcKT5gB",
    "pEUyLHTR1lLQon6US2q0BhkUpc5LMa",
    "PARogw3S76RRMTH1MaUTqwCeosy53h",
    "RzD4xO8QmDiqOdvBcmxuft3m4vkPeG",
    "Iv5zqx2rBMBhNNl9p7PlsqJU5PDb9Q",
    "MJmihkZ2Eguhz15Ts4eSHGafTEddE5",
];

lazy_static! {
    static ref BLOCKBERRY: Blockberry = Blockberry::new().unwrap();
}

pub async fn get_coin_decimals(coin_type: &str) -> Result<u8> {
    BLOCKBERRY.get_coin_decimals(coin_type).await
}

#[derive(Debug)]
struct Blockberry {
    client: reqwest::Client,
    key_count: usize,
    key_idx: Arc<AtomicUsize>,
}

impl Blockberry {
    fn new() -> Result<Self> {
        let client = reqwest::Client::builder().timeout(Duration::from_secs(30)).build()?;
        let key_count = BLOCKBERRY_API_KEYS.len();
        let key_idx = Arc::new(AtomicUsize::new(0));

        Ok(Self {
            client,
            key_count,
            key_idx,
        })
    }

    // [❗️WTF] rate limit: 1 req/15s
    async fn get_coin_decimals(&self, coin_type: &str) -> Result<u8> {
        // std::thread::sleep(Duration::from_secs(15));
        let url = format!("{}/{}", API_URL, coin_type);
        let api_key = self.get_api_key();
        info!(">> {}", url);
        let res = self.client.get(&url).header("x-api-key", api_key).send().await?;
        info!("<< {:?}", res);
        ensure!(res.status().is_success(), "blockberry status: {}", res.status());

        let resp = res.json::<serde_json::Value>().await?;
        let decimals = resp["decimals"].as_u64().ok_or_else(|| eyre!("decimals not found"))? as u8;

        Ok(decimals)
    }

    fn get_api_key(&self) -> &'static str {
        let mut idx = self.key_idx.fetch_add(1, Ordering::Relaxed);
        if idx >= self.key_count {
            idx = 0;
            self.key_idx.store(idx, Ordering::Relaxed);
        }

        BLOCKBERRY_API_KEYS[idx]
    }
}

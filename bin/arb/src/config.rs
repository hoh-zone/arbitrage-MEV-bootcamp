use std::collections::HashSet;

use sui_sdk::SUI_COIN_TYPE;

pub const GAS_BUDGET: u64 = 10_000_000_000;
pub const MAX_SQRT_PRICE_X64: u128 = 79226673515401279992447579055;
pub const MIN_SQRT_PRICE_X64: u128 = 4295048016;

pub fn pegged_coin_types() -> HashSet<&'static str> {
    HashSet::from_iter([
        SUI_COIN_TYPE,
        // USDC
        "0x5d4b302506645c37ff133b98c4b50a5ae14841659738d6d733d59d0d217a93bf::coin::COIN",
        // USDT
        "0xc060006111016b8a020ad5b33834984a437aaa7d3c74c18e09a95d48aceab08c::coin::COIN",
        // WETH
        "0xaf8cd5edc19c4512f4259f0bee101a40d41ebed738ade5874359610ef8eeced5::coin::COIN",
        // USDC
        "0xb231fcda8bbddb31f2ef02e6161444aec64a514e2c89279584ac9806ce9cf037::coin::COIN",
        // USDC
        "0xdba34672e30cb065b1f93e3ab55318768fd6fef66c15942c9f7cb846e2f900e7::usdc::USDC",
        // Bucket USD
        "0xce7ff77a83ea0cb6fd39bd8748e2ec89a3f41e8efdc3f4eb123e0ca37b184db2::buck::BUCK",
    ])
}

#[cfg(test)]
pub mod tests {

    pub const TEST_HTTP_URL: &str = "";
    pub const TEST_ATTACKER: &str = "";
}

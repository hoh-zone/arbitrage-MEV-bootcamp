use sui_types::{
    base_types::{ObjectID, SequenceNumber, SuiAddress},
    digests::{Digest, TransactionDigest},
};

const SCAN_URL: &str = "https://suiscan.xyz/mainnet";

// https://suiscan.xyz/mainnet/tx/WQ346mGc8sLjtcBPBfJNvTxCWar7U7Fsow9rTkmXgyE
pub fn tx(digest: &TransactionDigest, tag: Option<String>) -> String {
    format!(
        "[{tag_str}]({prefix}/tx/{digest})",
        tag_str = tag.unwrap_or_else(|| format!("{}", digest)),
        prefix = SCAN_URL,
        digest = digest,
    )
}

// https://suiscan.xyz/mainnet/object/0xb8d7d9e66a60c239e7a60110efcf8de6c705580ed924d0dde141f4a0e2c90105
pub fn object(object_id: ObjectID, tag: Option<String>) -> String {
    format!(
        "[{tag_str}]({prefix}/object/{object_id})",
        tag_str = tag.unwrap_or_else(|| format!("{}", object_id)),
        prefix = SCAN_URL,
        object_id = object_id,
    )
}

// https://suiscan.xyz/mainnet/account/0xac5bceec1b789ff840d7d4e6ce4ce61c90d190a7f8c4f4ddf0bff6ee2413c33c/portfolio
pub fn account(address: &SuiAddress, tag: Option<String>) -> String {
    format!(
        "[{tag}]({prefix}/account/{address}/portfolio)",
        tag = tag.unwrap_or_else(|| format!("{}", address)),
        address = address,
        prefix = SCAN_URL,
    )
}

// https://suiscan.xyz/mainnet/coin/0xa8816d3a6e3136e86bc2873b1f94a15cadc8af2703c075f2d546c2ae367f4df9::ocean::OCEAN/txs
pub fn coin(coin_type: &str, tag: Option<String>) -> String {
    format!(
        "[{tag}]({prefix}/coin/{coin_type}/txs)",
        tag = tag.unwrap_or_else(|| coin_type.to_string()),
        coin_type = coin_type,
        prefix = SCAN_URL,
    )
}

// https://suiscan.xyz/mainnet/checkpoint/AYWtSh7XWdBiRaEyh4oq3pxoaPmLkPJ6U1LBdwHuEXT
pub fn checkpoint(digest: &Digest, number: SequenceNumber) -> String {
    format!(
        "[{number}]({prefix}/checkpoint/{digest})",
        number = number,
        digest = digest,
        prefix = SCAN_URL,
    )
}

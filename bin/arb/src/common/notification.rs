use std::{fmt::Write, time::Duration};

use burberry::executor::telegram_message::{escape, Message, MessageBuilder};
use sui_types::digests::TransactionDigest;
use utils::{coin, link, telegram};

use crate::{arb::ArbResult, BUILD_VERSION};

const SUI_ARB_BOT_TOKEN: &str = "";
const GROUP_SUI_ARB: &str = "";
const THREAD_LOW_PROFIT: &str = "";
const THREAD_HIGH_PROFIT: &str = "";

pub fn new_tg_messages(
    digest: TransactionDigest,
    arb_digest: TransactionDigest,
    res: &ArbResult,
    elapsed: Duration,
    simulator_name: &str,
) -> Vec<Message> {
    let mut msg = String::with_capacity(4096);
    let trade_res = &res.best_trial_result;

    write!(
        msg,
        r#"*Profit*: `{profit}`

"#,
        profit = escape(&coin::format_sui_with_symbol(trade_res.profit)),
    )
    .unwrap();

    write!(
        msg,
        r#"*Digest*: {scan_link}
*Arb Digest*: {arb_scan_link}
*Coin*: {coin}
*Amount In*: {amount_in}
*Path*:
"#,
        scan_link = link::tx(&digest, None),
        arb_scan_link = link::tx(&arb_digest, None),
        coin = link::coin(&trade_res.coin_type, None),
        amount_in = escape(&coin::format_sui_with_symbol(trade_res.amount_in)),
    )
    .unwrap();

    for (i, dex) in trade_res.trade_path.path.iter().enumerate() {
        let tag = format!("{}({}-{})", dex.protocol(), dex.coin_in_type(), dex.coin_out_type());
        writeln!(
            msg,
            r#" {i}\. {dex}"#,
            dex = link::object(dex.object_id(), Some(escape(&tag)))
        )
        .unwrap();
    }

    writeln!(msg, "*Elapsed*: {}", escape(&format!("{:?}", elapsed))).unwrap();
    writeln!(
        msg,
        "*Elapsed TrialCtx Creation*: {}",
        escape(&format!("{:?}", res.create_trial_ctx_duration))
    )
    .unwrap();
    writeln!(
        msg,
        "*Elapsed Grid Search*: {}",
        escape(&format!("{:?}", res.grid_search_duration))
    )
    .unwrap();
    writeln!(msg, "*Elapsed GSS*: {}", escape(&format!("{:?}", res.gss_duration))).unwrap();
    writeln!(msg, "*Cache Misses*: {}", res.cache_misses).unwrap();
    writeln!(msg, "\n*{}*", simulator_name,).unwrap();
    writeln!(msg, "*{}*", escape(res.source.to_string().as_str())).unwrap();
    write!(msg, "*Version*: `{version}`", version = BUILD_VERSION).unwrap();

    println!("msg: {}", msg);

    let thread_id = if trade_res.profit > 1000000000 {
        "125670"
    } else {
        telegram::CHAT_MONEY_PRINTER_THREAD_TEST
    };

    let msg1 = MessageBuilder::new()
        .bot_token(telegram::R2D2_TELEGRAM_BOT_TOKEN)
        .chat_id(telegram::CHAT_MONEY_PRINTER)
        .thread_id(thread_id)
        .text(msg.clone())
        .disable_link_preview(true)
        .build();

    // SUI Arbitrage Group
    let thread_id = if trade_res.profit > 1000000000 {
        THREAD_HIGH_PROFIT
    } else {
        THREAD_LOW_PROFIT
    };
    let msg2 = MessageBuilder::new()
        .bot_token(SUI_ARB_BOT_TOKEN)
        .chat_id(GROUP_SUI_ARB)
        .thread_id(thread_id)
        .text(msg.clone())
        .disable_link_preview(true)
        .build();

    vec![msg1, msg2]
}

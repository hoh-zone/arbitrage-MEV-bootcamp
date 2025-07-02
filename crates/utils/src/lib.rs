pub mod coin;
pub mod heartbeat;
pub mod link;
pub mod object;
pub mod telegram;

use burberry::executor::telegram_message::{escape, MessageBuilder, TelegramMessageDispatcher};
use sui_sdk::{SuiClient, SuiClientBuilder};
use tokio::runtime::{Builder, Handle, RuntimeFlavor};
use tracing::error;

use crate::telegram::*;

pub fn set_panic_hook() {
    std::panic::set_hook(Box::new(move |info| {
        // replace any arg that is longer than 40 chars with "[REDACTED]"
        // (e.g. private keys)
        let cmdline = std::env::args()
            .map(|arg| if arg.len() > 32 { "[REDACTED]".to_string() } else { arg })
            .collect::<Vec<_>>()
            .join(" ");

        let thread = std::thread::current();
        let thread = thread.name().unwrap_or("<unnamed>");

        let msg = match info.payload().downcast_ref::<&'static str>() {
            Some(s) => *s,
            None => match info.payload().downcast_ref::<String>() {
                Some(s) => &**s,
                None => "Box<Any>",
            },
        };

        let err_msg = match info.location() {
            Some(location) => {
                format!(
                    "thread '{}' panicked at '{}': {}:{}",
                    thread,
                    msg,
                    location.file(),
                    location.line(),
                )
            }
            None => format!("thread '{}' panicked at '{}'", thread, msg,),
        };

        send_panic_to_telegram(&cmdline, &err_msg);
        error!(target: "panic_hook", err_msg);
    }));
}

fn send_panic_to_telegram(cmdline: &str, msg: &str) {
    let telegram_dispatcher = TelegramMessageDispatcher::new(None, None, None);
    let escaped = escape(&format!("cmd: {:?}\nerror: {:?}", cmdline, msg));
    let msg = MessageBuilder::new()
        .bot_token(R2D2_TELEGRAM_BOT_TOKEN)
        .chat_id(CHAT_MONEY_PRINTER)
        .thread_id(CHAT_MONEY_PRINTER_THREAD_ERROR_REPORT)
        .text(&escaped)
        .disable_link_preview(true)
        .disable_notification(true)
        .build();

    match Handle::try_current() {
        Ok(handle) => match handle.runtime_flavor() {
            RuntimeFlavor::CurrentThread => std::thread::scope(move |s| {
                s.spawn(move || {
                    Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .unwrap()
                        .block_on(async move {
                            telegram_dispatcher.send_message(msg).await;
                        })
                })
                .join()
                .unwrap()
            }),
            _ => tokio::task::block_in_place(move || {
                handle.block_on(async move {
                    telegram_dispatcher.send_message(msg).await;
                })
            }),
        },
        Err(_) => Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async move {
                telegram_dispatcher.send_message(msg).await;
            }),
    }
}

pub fn current_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

pub async fn new_test_sui_client() -> SuiClient {
    SuiClientBuilder::default()
        .build("")
        .await
        .unwrap()
}

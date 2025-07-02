use std::fmt::Display;

pub use tracing::level_filters::LevelFilter;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer};

pub fn init<T: Into<String>>(name: T) {
    let console_layer = fmt::layer().with_target(false).with_filter(EnvFilter::new("info"));

    let file_appender = tracing_appender::rolling::hourly("./logs/", format!("{}.log", name.into()));
    let file_layer = fmt::layer()
        .with_ansi(false)
        .with_writer(file_appender)
        .with_target(true)
        .with_filter(EnvFilter::new("info"));

    tracing_subscriber::registry()
        .with(console_layer)
        .with(file_layer)
        .init();
}

pub fn init_with_chain<T: Display>(chain: T, name: String) {
    init(format!("{name}-{chain}"));
}

pub fn new_whitelist_mode_env_filter(allowed_modules: &[&str], level: LevelFilter) -> EnvFilter {
    let directives = allowed_modules
        .iter()
        .map(|module| {
            if module.contains("=") {
                module.to_string()
            } else {
                format!("{}={}", module, level)
            }
        })
        .collect::<Vec<String>>()
        .join(",");

    EnvFilter::builder()
        .with_default_directive(LevelFilter::OFF.into())
        .parse(&directives)
        .unwrap()
}

pub fn init_with_whitelisted_modules<T: Display>(chain: T, name: String, modules: &[&str]) {
    let modules = ["burberry", "reconstruct", "mev_core::flashloan", "panic_hook"]
        .iter()
        .chain(modules.iter())
        .cloned()
        .collect::<Vec<_>>();

    let console_layer = fmt::layer()
        .with_target(true)
        .with_filter(new_whitelist_mode_env_filter(&modules, LevelFilter::INFO));

    let file_appender = tracing_appender::rolling::hourly("./logs/", format!("{name}-{chain}.log"));

    let file_layer = fmt::layer()
        .with_ansi(false)
        .with_writer(file_appender)
        .with_target(true)
        .with_filter(new_whitelist_mode_env_filter(&modules, LevelFilter::TRACE));

    tracing_subscriber::registry()
        .with(file_layer)
        .with(console_layer)
        .init();
}

pub fn init_console_logger(level: Option<LevelFilter>) {
    init_console_logger_with_directives(level, &[]);
}

pub fn init_console_logger_with_directives(level: Option<LevelFilter>, directives: &[&str]) {
    let mut env_filter = EnvFilter::builder()
        .with_default_directive(level.unwrap_or(LevelFilter::INFO).into())
        .from_env()
        .unwrap();

    for directive in directives {
        env_filter = env_filter.add_directive(directive.parse().unwrap());
    }

    tracing_subscriber::registry()
        .with(fmt::layer().with_timer(fmt::time::SystemTime))
        .with(env_filter)
        .init();
}

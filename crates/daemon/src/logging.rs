use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

pub fn init() {
    let filter = EnvFilter::try_from_env("INPUTSYNC_LOG")
        .unwrap_or_else(|_| EnvFilter::new("info,inputsync_daemon=debug"));

    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_target(true).with_thread_names(false))
        .init();
}

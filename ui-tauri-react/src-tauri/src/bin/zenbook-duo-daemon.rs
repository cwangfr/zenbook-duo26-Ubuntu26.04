#[tokio::main]
async fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    if let Err(err) = zenbook_duo_control_lib::runtime::daemon::run().await {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

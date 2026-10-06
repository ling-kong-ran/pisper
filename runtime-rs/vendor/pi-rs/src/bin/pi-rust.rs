//! Native compatibility CLI; `pirs` remains the original M1 development CLI.
use pi_rust::coding_agent::{
    cli::setup::setup_cli,
    main::{entry::run_cli, options::is_truthy_env_flag},
};
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Do process-global environment setup before starting worker threads.
    if args.iter().any(|arg| arg == "--offline")
        || is_truthy_env_flag(std::env::var("PI_OFFLINE").ok().as_deref())
    {
        std::env::set_var("PI_OFFLINE", "1");
        std::env::set_var("PI_SKIP_VERSION_CHECK", "1");
    }
    setup_cli();
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("Error: {error}");
            std::process::exit(1);
        }
    };
    let code = match runtime.block_on(run_cli(&args)) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("Error: {error}");
            1
        }
    };
    // Tokio stdin uses blocking reads; a one-shot command must not wait for a
    // terminal read or a badly behaved extension task after its mode is done.
    runtime.shutdown_timeout(std::time::Duration::from_millis(100));
    std::process::exit(code);
}

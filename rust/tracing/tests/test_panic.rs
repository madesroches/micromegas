#[test]
fn test_panic() {
    std::panic::set_hook(Box::new(|panic_info| {
        println!("{:?}", panic_info);
        #[allow(clippy::exit)]
        std::process::exit(0);
    }));
    lgn_tracing::panic_hook::init_panic_hook();
    panic!("PANIC!!");
}

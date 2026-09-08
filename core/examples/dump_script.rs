fn main() {
    let cfg = portico_core::config::Config::load();
    let which = std::env::args().nth(1).unwrap_or_default();
    if which == "uninstall" {
        print!("{}", portico_core::setup::uninstall_script());
    } else {
        print!("{}", portico_core::setup::install_script(&cfg));
    }
}

fn main() {
    let cfg = portico_core::config::Config::load();
    let which = std::env::args().nth(1).unwrap_or_default();
    if which == "caddyfile" {
        print!("{}", portico_core::caddy::render(&cfg));
    } else if which == "loopback" {
        print!("{}", portico_core::setup::loopback_script());
    } else if which == "uninstall" {
        print!("{}", portico_core::setup::uninstall_script());
    } else {
        print!("{}", portico_core::setup::install_script(&cfg));
    }
}

//! Embeds the screen-tools-rs icon into the Windows executable.

fn main() {
    if std::env::var_os("CARGO_CFG_WINDOWS").is_none() {
        return;
    }
    winresource::WindowsResource::new()
        .set_icon("assets/screen-tools-rs.ico")
        .compile()
        .expect("cannot compile the icon resource");
}

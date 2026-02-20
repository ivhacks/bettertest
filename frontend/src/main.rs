mod app;

#[cfg(test)]
mod frontend_tests;

fn main() {
    let path = web_sys::window()
        .and_then(|w| w.location().pathname().ok())
        .unwrap_or_default();

    if path == "/logs" {
        yew::Renderer::<app::LogsPage>::new().render();
    } else {
        yew::Renderer::<app::App>::new().render();
    }
}

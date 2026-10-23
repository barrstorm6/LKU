#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod config;
mod k8s;
mod logs;
mod pods;
mod theme;
mod ui;

fn main() -> eframe::Result {
    use tracing_subscriber::{Layer, layer::SubscriberExt, util::SubscriberInitExt};
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer().with_filter(tracing_subscriber::filter::filter_fn(
                |metadata| {
                    metadata.target().starts_with("Kube")
                        || metadata.target().starts_with("kube_desktop")
                },
            )),
        )
        .init();
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(_) => {
            rfd::MessageDialog::new()
                .set_title("Kube")
                .set_description("Could not start background workers. Restart Kube.")
                .set_level(rfd::MessageLevel::Error)
                .show();
            return Ok(());
        }
    };
    let mut options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([760.0, 620.0])
            .with_min_inner_size([480.0, 360.0]),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    if let eframe::egui_wgpu::WgpuSetup::CreateNew(setup) = &mut options.wgpu_options.wgpu_setup {
        setup.power_preference = wgpu::PowerPreference::LowPower;
        let default_descriptor = setup.device_descriptor.clone();
        setup.device_descriptor = std::sync::Arc::new(move |adapter| {
            let mut descriptor = default_descriptor(adapter);
            descriptor.memory_hints = wgpu::MemoryHints::MemoryUsage;
            descriptor
        });
    }
    let runtime = std::sync::Arc::new(runtime);
    let app_runtime = runtime.clone();
    let result = eframe::run_native(
        "Kube",
        options,
        Box::new(move |cc| {
            theme::apply(&cc.egui_ctx);
            Ok(Box::new(app::App::new(app_runtime, cc.egui_ctx.clone())))
        }),
    );
    // kube-rs credential plugins and native pickers may use blocking workers.
    // A misbehaving external program must not keep the window/process alive forever.
    if let Ok(runtime) = std::sync::Arc::try_unwrap(runtime) {
        runtime.shutdown_timeout(std::time::Duration::from_millis(500));
    }
    result
}

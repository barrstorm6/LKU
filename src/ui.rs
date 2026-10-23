use crate::{
    app::{App, Status},
    pods::Health,
    theme,
};
use egui::{Align, Color32, Layout, RichText};

fn dot(ui: &mut egui::Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 16.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 3.0, color);
}

pub fn render(app: &mut App, root: &mut egui::Ui) {
    egui::CentralPanel::default()
        .frame(egui::Frame::new().fill(Color32::WHITE).inner_margin(20))
        .show(root, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Kube").size(20.0).strong());
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if !app.settings.clusters.is_empty() {
                        let current = app
                            .current()
                            .map(|c| c.name.clone())
                            .unwrap_or_else(|| "Select cluster".into());
                        let mut select = None;
                        let mut remove = None;
                        let mut add = false;
                        egui::ComboBox::from_id_salt("connections")
                            .selected_text(current)
                            .show_ui(ui, |ui| {
                                for connection in &app.settings.clusters {
                                    ui.horizontal(|ui| {
                                        if ui
                                            .selectable_label(
                                                app.settings.last_cluster_id.as_ref()
                                                    == Some(&connection.id),
                                                &connection.name,
                                            )
                                            .on_hover_text(format!(
                                                "{}\n{}",
                                                connection.kubeconfig_path.display(),
                                                connection.context
                                            ))
                                            .clicked()
                                        {
                                            select = Some(connection.id.clone());
                                        }
                                        if ui
                                            .small_button("×")
                                            .on_hover_text("Remove saved connection")
                                            .clicked()
                                        {
                                            remove = Some(connection.id.clone());
                                        }
                                    });
                                }
                                ui.separator();
                                add = ui.button("+ Add cluster").clicked();
                            });
                        dot(
                            ui,
                            match app.status {
                                Status::Connected => theme::GREEN,
                                Status::Error => theme::RED,
                                Status::Unknown => theme::GRAY,
                            },
                        );
                        if let Some(id) = select {
                            app.select_connection(&id);
                        }
                        if let Some(id) = remove {
                            app.remove_connection(&id);
                        }
                        if add {
                            app.pick_file(None);
                        }
                    }
                });
            });
            feedback(app, ui);
            if app.settings.clusters.is_empty() && app.candidate.is_none() && !app.loading {
                ui.add_space((ui.available_height() / 2.0 - 25.0).max(0.0));
                ui.vertical_centered(|ui| {
                    if ui
                        .add_enabled(
                            !app.picking,
                            egui::Button::new(if app.picking {
                                "Selecting…"
                            } else {
                                "+ Add cluster"
                            }),
                        )
                        .clicked()
                    {
                        app.pick_file(None);
                    }
                });
            } else {
                ui.add_space(12.0);
                ui.separator();
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Pods").strong().size(16.0));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if app.restricted {
                            let submit = ui.button("Go").clicked();
                            let edit = ui.add(
                                egui::TextEdit::singleline(&mut app.manual_namespace)
                                    .desired_width(180.0)
                                    .hint_text("Namespace"),
                            );
                            if submit
                                || (edit.lost_focus()
                                    && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                            {
                                app.change_namespace(app.manual_namespace.trim().to_owned());
                            }
                            ui.label("Namespace").on_hover_text(
                                "Namespace listing is forbidden. Enter a namespace you can access.",
                            );
                        } else {
                            let mut namespace = app.namespace.clone();
                            egui::ComboBox::from_id_salt("namespace")
                                .selected_text(if namespace.is_empty() {
                                    "Namespace"
                                } else {
                                    &namespace
                                })
                                .show_ui(ui, |ui| {
                                    for name in &app.namespaces {
                                        ui.selectable_value(&mut namespace, name.clone(), name);
                                    }
                                });
                            if namespace != app.namespace {
                                app.change_namespace(namespace);
                            }
                        }
                    });
                });
                if ui
                    .add(
                        egui::TextEdit::singleline(&mut app.search)
                            .hint_text("Search pods")
                            .desired_width(f32::INFINITY),
                    )
                    .changed()
                {
                    app.filter();
                }
                ui.add_space(4.0);
                if app.loading {
                    ui.label(RichText::new("Connecting…").color(theme::GRAY));
                } else if app
                    .error
                    .as_ref()
                    .is_some_and(|e| e.starts_with("Access denied"))
                {
                    ui.label("Access denied");
                } else if app.filtered.is_empty() && app.error.is_none() {
                    ui.label(if app.pods.live.is_empty() {
                        "No pods"
                    } else {
                        "No results"
                    });
                } else {
                    let row_height = 30.0;
                    egui::ScrollArea::vertical()
                        .id_salt("pods")
                        .auto_shrink([false, false])
                        .show_rows(ui, row_height, app.filtered.len(), |ui, range| {
                            for index in range {
                                let Some(pod) = app.pods.live.get(&app.filtered[index]) else {
                                    continue;
                                };
                                let color = match pod.health {
                                    Health::Healthy => theme::GREEN,
                                    Health::Unhealthy => theme::RED,
                                    Health::Transitional => theme::GRAY,
                                };
                                ui.push_id(&pod.uid, |ui| {
                                    let (rect, response) = ui.allocate_exact_size(
                                        egui::vec2(ui.available_width(), row_height),
                                        egui::Sense::click(),
                                    );
                                    if response.hovered() {
                                        ui.painter().rect_filled(rect, 2.0, theme::LIGHT);
                                    }
                                    ui.painter().circle_filled(
                                        egui::pos2(rect.left() + 5.0, rect.center().y),
                                        3.0,
                                        color,
                                    );
                                    let status_width = if pod.health == Health::Healthy {
                                        0.0
                                    } else {
                                        150.0
                                    };
                                    let text_rect = egui::Rect::from_min_max(
                                        egui::pos2(rect.left() + 18.0, rect.top()),
                                        egui::pos2(rect.right() - status_width, rect.bottom()),
                                    );
                                    let painter = ui.painter().with_clip_rect(text_rect);
                                    painter.text(
                                        egui::pos2(text_rect.left(), rect.center().y),
                                        egui::Align2::LEFT_CENTER,
                                        &pod.name,
                                        egui::FontId::proportional(14.0),
                                        theme::INK,
                                    );
                                    if status_width > 0.0 {
                                        ui.painter().text(
                                            egui::pos2(rect.right() - 4.0, rect.center().y),
                                            egui::Align2::RIGHT_CENTER,
                                            &pod.status,
                                            egui::FontId::proportional(12.0),
                                            color,
                                        );
                                    }
                                    if response.clicked() {
                                        app.selected_pod = Some(pod.name.clone());
                                    }
                                });
                            }
                        });
                }
            }
        });
    candidate(app, root.ctx());
    details(app, root.ctx());
    log_viewer(app, root.ctx());
}

fn feedback(app: &mut App, ui: &mut egui::Ui) {
    if let Some(error) = app.error.clone() {
        ui.colored_label(theme::RED, error);
        ui.horizontal(|ui| {
            if ui.button("Retry").clicked() {
                app.retry();
            }
            if app.current().is_some() && ui.button("Choose kubeconfig…").clicked() {
                app.pick_file(app.settings.last_cluster_id.clone());
            }
        });
    }
    if let Some(error) = &app.settings_error {
        ui.colored_label(theme::RED, error);
    }
}

fn candidate(app: &mut App, ctx: &egui::Context) {
    let Some(candidate) = &mut app.candidate else {
        return;
    };
    let mut open = true;
    let mut connect = false;
    egui::Window::new("Add cluster")
        .collapsible(false)
        .resizable(false)
        .default_width(360.0)
        .open(&mut open)
        .show(ctx, |ui| {
            ui.label(
                candidate
                    .path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy(),
            )
            .on_hover_text(candidate.path.display().to_string());
            egui::ComboBox::from_id_salt("context")
                .selected_text(&candidate.selected)
                .show_ui(ui, |ui| {
                    for name in &candidate.contexts {
                        if ui
                            .selectable_value(&mut candidate.selected, name.clone(), name)
                            .changed()
                        {
                            app.trust = false;
                        }
                    }
                });
            let valid = match candidate.validate() {
                Ok(Some(command)) => {
                    ui.label(
                        "This kubeconfig runs a credential program with your user permissions:",
                    );
                    ui.monospace(command);
                    ui.checkbox(
                        &mut app.trust,
                        "I trust this kubeconfig and credential program",
                    );
                    app.trust
                }
                Ok(None) => true,
                Err(error) => {
                    ui.colored_label(theme::RED, error);
                    false
                }
            };
            connect = ui
                .add_enabled(valid, egui::Button::new("Connect"))
                .clicked();
        });
    if !open {
        app.candidate = None;
        app.pending_connection = None;
        app.loading = false;
    } else if connect {
        app.connect_candidate();
    }
}

fn details(app: &mut App, ctx: &egui::Context) {
    if app.viewer.is_some() {
        return;
    }
    let Some(name) = &app.selected_pod else {
        return;
    };
    let mut open = true;
    let mut logs = None;
    egui::Window::new("Pod")
        .id(egui::Id::unique("details"))
        .collapsible(false)
        .open(&mut open)
        .default_width(450.0)
        .show(ctx, |ui| {
            let Some(pod) = app.pods.live.get(name) else {
                ui.label("Pod no longer exists");
                return;
            };
            ui.label(RichText::new(&pod.name).strong());
            ui.separator();
            egui::Grid::new("pod_metadata")
                .spacing([22.0, 8.0])
                .show(ui, |ui| {
                    for (label, value) in [
                        ("Namespace", pod.namespace.clone()),
                        ("Status", pod.status.clone()),
                        ("Ready containers", format!("{} / {}", pod.ready, pod.total)),
                        ("Restarts", pod.restarts.to_string()),
                        ("Pod IP", pod.ip.clone()),
                        ("Node", pod.node.clone()),
                        ("Containers", pod.containers.join(", ")),
                    ] {
                        ui.label(RichText::new(label).color(theme::GRAY));
                        ui.label(if value.is_empty() { "—" } else { &value });
                        ui.end_row();
                    }
                });
            ui.add_space(8.0);
            if ui
                .add_enabled(!pod.containers.is_empty(), egui::Button::new("Logs"))
                .clicked()
            {
                logs = Some((pod.name.clone(), pod.containers[0].clone()));
            }
        });
    if !open {
        app.selected_pod = None;
    }
    if let Some((pod, container)) = logs {
        app.open_logs(pod, container);
    }
}

fn log_viewer(app: &mut App, ctx: &egui::Context) {
    let Some(viewer) = &mut app.viewer else {
        return;
    };
    let mut open = true;
    let mut restart = false;
    egui::Window::new("Logs")
        .id(egui::Id::unique("logs"))
        .collapsible(false)
        .open(&mut open)
        .default_size([680.0, 440.0])
        .min_size([400.0, 260.0])
        .show(ctx, |ui| {
            ui.label(RichText::new(&viewer.pod).strong());
            let exists = app.pods.live.get(&viewer.pod);
            if let Some(pod) = exists {
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt("container")
                        .selected_text(&viewer.container)
                        .show_ui(ui, |ui| {
                            for name in &pod.containers {
                                restart |= ui
                                    .selectable_value(&mut viewer.container, name.clone(), name)
                                    .changed();
                            }
                        });
                    restart |= ui.checkbox(&mut viewer.timestamps, "Timestamps").changed();
                    ui.checkbox(&mut viewer.follow, "Auto-scroll");
                    if ui.small_button("Reconnect").clicked() {
                        restart = true;
                    }
                });
            } else {
                ui.colored_label(theme::GRAY, "Pod no longer exists");
            }
            ui.add(
                egui::TextEdit::singleline(&mut viewer.query)
                    .hint_text("Search visible logs")
                    .desired_width(f32::INFINITY),
            );
            if let Some(error) = &viewer.error {
                ui.colored_label(theme::RED, error);
            }
            viewer.filter();
            // Only visible lines are laid out. Read-only text editors allow native text selection and Ctrl+C.
            let height = ui.text_style_height(&egui::TextStyle::Monospace) + 6.0;
            egui::ScrollArea::both()
                .id_salt("log_lines")
                .auto_shrink([false, false])
                .stick_to_bottom(viewer.follow)
                .show_rows(ui, height, viewer.matches.len(), |ui, range| {
                    for row in range {
                        let index = viewer.matches[row];
                        let Some(line) = viewer.buffer.lines.get(index) else {
                            continue;
                        };
                        let mut text: &str = line;
                        let absolute = viewer.buffer.total_lines - viewer.buffer.lines.len() as u64
                            + index as u64;
                        ui.push_id(absolute, |ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut text)
                                    .font(egui::TextStyle::Monospace)
                                    .frame(egui::Frame::NONE)
                                    .desired_width(ui.available_width().max(600.0)),
                            );
                        });
                    }
                });
        });
    if !open {
        app.close_logs();
    } else if restart {
        app.restart_logs();
    }
}

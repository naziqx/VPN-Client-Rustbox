//! Main window: menu · groups sidebar · connection card · profile table ·
//! (optional) log · status bar.

use eframe::egui::{self, Frame, Margin, RichText, Sense};
use egui_extras::{Column, TableBuilder};
use rustbox_core::model::{Group, Latency, Profile};
use rustbox_core::storage::DEFAULT_GROUP;
use rustbox_core::{GroupId, ProfileId};

use crate::app::{Mode, RustBoxApp, SortKey, unix_now};
use crate::dialogs::{EditDialog, SubscriptionDialog};
use crate::theme::{self, Palette};
use rustbox_core::settings::TestKind;

pub fn show(app: &mut RustBoxApp, ui: &mut egui::Ui) {
    handle_shortcuts(app, ui.ctx());
    let p = Palette::of(ui);

    egui::Panel::top("menu")
        .frame(
            Frame::new()
                .fill(p.sidebar)
                .inner_margin(Margin::symmetric(8, 2)),
        )
        .show(ui, |ui| menu_bar(app, ui));
    egui::Panel::bottom("status")
        .frame(
            Frame::new()
                .fill(p.sidebar)
                .inner_margin(Margin::symmetric(12, 3)),
        )
        .show(ui, |ui| status_bar(app, ui));
    if app.show_logs {
        egui::Panel::bottom("logs")
            .resizable(true)
            .default_size(180.0)
            .min_size(80.0)
            .frame(
                Frame::new()
                    .fill(p.bg)
                    .inner_margin(Margin::symmetric(12, 8)),
            )
            .show(ui, |ui| log_panel(app, ui));
    }
    egui::Panel::left("groups")
        .resizable(true)
        .default_size(230.0)
        .min_size(190.0)
        .frame(Frame::new().fill(p.sidebar).inner_margin(Margin::same(12)))
        .show(ui, |ui| sidebar(app, ui));
    egui::CentralPanel::no_frame()
        .frame(Frame::new().fill(p.bg).inner_margin(Margin::same(16)))
        .show(ui, |ui| {
            connection_card(app, ui);
            ui.add_space(12.0);
            toolbar(app, ui);
            ui.add_space(6.0);
            profiles_table(app, ui);
        });
}

fn handle_shortcuts(app: &mut RustBoxApp, ctx: &egui::Context) {
    if ctx.egui_wants_keyboard_input() || app.dialogs.any_open() {
        return;
    }
    let (paste, delete, select_all, enter) = ctx.input(|i| {
        let paste = i.events.iter().find_map(|e| match e {
            egui::Event::Paste(text) => Some(text.clone()),
            _ => None,
        });
        (
            paste,
            i.key_pressed(egui::Key::Delete),
            i.modifiers.command && i.key_pressed(egui::Key::A),
            i.key_pressed(egui::Key::Enter),
        )
    });
    if let Some(text) = paste {
        app.import_text(&text);
    }
    if delete && !app.selection.is_empty() {
        app.dialogs.confirm_delete = Some(app.selection.iter().copied().collect());
    }
    if select_all {
        app.selection = app.visible_profiles().iter().map(|p| p.id).collect();
    }
    if enter && let Some(&id) = app.selection.iter().next() {
        app.connect(id);
    }
}

// ── Menu ──────────────────────────────────────────────────────

fn menu_bar(app: &mut RustBoxApp, ui: &mut egui::Ui) {
    egui::MenuBar::new().ui(ui, |ui| {
        ui.menu_button("Программа", |ui| {
            if ui.button("Настройки…").clicked() {
                app.dialogs.settings = Some(app.settings.clone());
                ui.close();
            }
            let mut logs = app.show_logs;
            if ui.checkbox(&mut logs, "Показывать лог").clicked() {
                app.toggle_logs();
                ui.close();
            }
            ui.separator();
            if ui.button("Выход").clicked() {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            }
        });
        ui.menu_button("Профили", |ui| add_menu_items(app, ui));
        ui.menu_button("Тест", |ui| test_menu_items(app, ui));
    });
}

fn add_menu_items(app: &mut RustBoxApp, ui: &mut egui::Ui) {
    if ui
        .add(egui::Button::new("Вставить из буфера обмена").shortcut_text("Ctrl+V"))
        .clicked()
    {
        app.import_clipboard();
        ui.close();
    }
    if ui.button("Импорт ссылок…").clicked() {
        app.dialogs.import = Some(String::new());
        ui.close();
    }
    if ui.button("Новый профиль вручную…").clicked() {
        app.dialogs.edit = Some(EditDialog::new_profile());
        ui.close();
    }
    ui.separator();
    if ui.button("Добавить подписку…").clicked() {
        app.dialogs.subscription = Some(SubscriptionDialog::default());
        ui.close();
    }
    if ui.button("Обновить все подписки").clicked() {
        app.update_all_subscriptions();
        ui.close();
    }
}

fn test_menu_items(app: &mut RustBoxApp, ui: &mut egui::Ui) {
    let scope = if app.selection.len() > 1 {
        "выбранные"
    } else {
        "все в группе"
    };
    if ui
        .button(format!("URL-тест: {scope}"))
        .on_hover_text("Реальный запрос через сервер — показывает, работает ли он")
        .clicked()
    {
        let p = app.selected_or_visible();
        app.url_test(p);
        ui.close();
    }
    if ui
        .button(format!("TCP ping: {scope}"))
        .on_hover_text("Только доступность порта — быстро, но не гарантирует работу")
        .clicked()
    {
        let p = app.selected_or_visible();
        app.tcp_ping(p);
        ui.close();
    }
    ui.separator();
    if ui.button("Сортировать по задержке").clicked() {
        app.sort = Some((SortKey::Latency, true));
        ui.close();
    }
    if ui.button("Сохранить текущий порядок").clicked() {
        let order: Vec<ProfileId> = app.visible_profiles().iter().map(|p| p.id).collect();
        app.store.reorder(app.current_group, &order);
        app.sort = None;
        app.save_store();
        ui.close();
    }
    if ui.button("Сбросить результаты").clicked() {
        let group = app.current_group;
        for p in app.store.profiles.iter_mut().filter(|p| p.group == group) {
            p.latency = None;
        }
        app.save_store();
        ui.close();
    }
    if ui.button("Удалить недоступные…").clicked() {
        let dead: Vec<ProfileId> = app
            .store
            .profiles_in(app.current_group)
            .filter(|p| matches!(p.latency, Some(Latency::Error(_))))
            .map(|p| p.id)
            .collect();
        if dead.is_empty() {
            app.info("недоступных профилей нет");
        } else {
            app.dialogs.confirm_delete = Some(dead);
        }
        ui.close();
    }
}

// ── Sidebar ───────────────────────────────────────────────────

/// "Default" is an internal name; users see a friendlier one.
pub fn group_name(g: &Group) -> String {
    if g.id == DEFAULT_GROUP && g.name == "Default" {
        "Мои профили".into()
    } else {
        g.name.clone()
    }
}

fn sidebar(app: &mut RustBoxApp, ui: &mut egui::Ui) {
    let p = Palette::of(ui);
    ui.horizontal(|ui| {
        theme::section_title(ui, "Группы");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add(egui::Button::new(RichText::new("+").size(17.0)).frame_when_inactive(false))
                .on_hover_text("Добавить подписку")
                .clicked()
            {
                app.dialogs.subscription = Some(SubscriptionDialog::default());
            }
        });
    });
    ui.add_space(2.0);

    let groups: Vec<(GroupId, String, bool)> = app
        .store
        .groups
        .iter()
        .map(|g| (g.id, group_name(g), g.subscription.is_some()))
        .collect();

    // The subscription card is drawn from the bottom; the list takes the rest.
    let current_sub = app
        .store
        .group(app.current_group)
        .and_then(|g| g.subscription.clone());
    let card_height = if current_sub.is_some() { 150.0 } else { 0.0 };

    egui::ScrollArea::vertical()
        .id_salt("groups_scroll")
        .max_height((ui.available_height() - card_height).max(60.0))
        .auto_shrink([false, true])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            for (id, name, is_sub) in groups {
                let count = app.store.profiles_in(id).count();
                let selected = app.current_group == id;
                let right = if app.updating.contains(&id) {
                    RichText::new("⟳").color(p.accent)
                } else {
                    RichText::new(count.to_string()).color(p.muted)
                };
                let resp = ui.add_sized(
                    [ui.available_width(), 30.0],
                    egui::Button::selectable(selected, name)
                        .right_text(right)
                        .truncate(),
                );
                if resp.clicked() && !selected {
                    app.current_group = id;
                    app.selection.clear();
                }
                resp.context_menu(|ui| group_menu(app, ui, id, is_sub));
            }
        });

    if let Some(sub) = current_sub {
        ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
            subscription_card(app, ui, &sub);
        });
    }
}

fn group_menu(app: &mut RustBoxApp, ui: &mut egui::Ui, id: GroupId, is_sub: bool) {
    if is_sub && ui.button("Обновить подписку").clicked() {
        app.update_subscription(id);
        ui.close();
    }
    if ui.button("Редактировать…").clicked() {
        app.dialogs.subscription = SubscriptionDialog::edit(&app.store, id);
        ui.close();
    }
    if ui.button("Скопировать все ссылки").clicked() {
        let profiles: Vec<Profile> = app.store.profiles_in(id).cloned().collect();
        app.copy_links(ui.ctx(), &profiles);
        ui.close();
    }
    if id != DEFAULT_GROUP {
        ui.separator();
        if ui
            .button(RichText::new("Удалить группу…").color(Palette::of(ui).bad))
            .clicked()
        {
            app.dialogs.confirm_delete_group = Some(id);
            ui.close();
        }
    }
}

fn subscription_card(
    app: &mut RustBoxApp,
    ui: &mut egui::Ui,
    sub: &rustbox_core::model::Subscription,
) {
    let p = Palette::of(ui);
    let group = app.current_group;
    // bottom_up layout: draw the card as one block so its content stays top-down.
    ui.with_layout(egui::Layout::top_down(egui::Align::LEFT), |ui| {
        theme::card(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                theme::section_title(ui, "Подписка");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let updating = app.updating.contains(&group);
                    if updating {
                        ui.spinner();
                    } else if ui
                        .add(egui::Button::new("⟳").frame_when_inactive(false))
                        .on_hover_text("Обновить подписку")
                        .clicked()
                    {
                        app.update_subscription(group);
                    }
                });
            });

            if let Some(info) = &sub.info {
                let used = info.upload + info.download;
                if info.total > 0 {
                    let frac = (used as f32 / info.total as f32).min(1.0);
                    ui.label(format!(
                        "{} из {}",
                        human_bytes(used),
                        human_bytes(info.total)
                    ));
                    let color = if frac > 0.9 {
                        p.bad
                    } else if frac > 0.75 {
                        p.warn
                    } else {
                        p.accent
                    };
                    ui.add(
                        egui::ProgressBar::new(frac)
                            .desired_height(6.0)
                            .corner_radius(3)
                            .fill(color),
                    );
                } else {
                    ui.label(format!("Использовано {}", human_bytes(used)));
                }
                if info.expire > 0 {
                    let days = (info.expire as i64 - unix_now() as i64) / 86400;
                    let (text, color) = match days {
                        ..0 => ("Срок истёк".to_string(), p.bad),
                        0 => ("Истекает сегодня".to_string(), p.bad),
                        1..=3 => (format!("Осталось {}", days_word(days)), p.warn),
                        _ => (format!("Осталось {}", days_word(days)), p.text),
                    };
                    ui.label(RichText::new(text).color(color));
                }
            }
            if let Some(t) = sub.last_updated {
                let mins = unix_now().saturating_sub(t) / 60;
                ui.label(
                    RichText::new(format!("Обновлено {}", human_minutes(mins)))
                        .small()
                        .color(p.muted),
                );
            } else {
                ui.label(RichText::new("Ещё не обновлялась").small().color(p.muted));
            }
        });
    });
}

// ── Connection card ───────────────────────────────────────────

fn connection_card(app: &mut RustBoxApp, ui: &mut egui::Ui) {
    let p = Palette::of(ui);
    let connected = app.connected_profile();
    let target = connected
        .or_else(|| app.selection.iter().next().copied())
        .or(app.settings.selected)
        .and_then(|id| app.store.profile(id))
        .cloned();

    let (state, color) = if app.connecting && connected.is_none() {
        ("Подключение…", p.warn)
    } else if app.connecting {
        ("Переключение…", p.warn)
    } else if connected.is_some() {
        ("Защищено", p.ok)
    } else {
        ("Не подключено", p.muted)
    };

    theme::card(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    theme::dot(ui, color, 4.5);
                    ui.label(RichText::new(state).color(color).strong());
                    if let (Some(at), Some(_)) = (app.connected_at, connected) {
                        ui.label(
                            RichText::new(format_uptime(at.elapsed().as_secs())).color(p.muted),
                        );
                    }
                });
                match &target {
                    Some(t) => {
                        ui.label(RichText::new(t.display_name()).size(20.0).strong());
                        let mut details = vec![t.type_label(), t.address()];
                        if let Some(Latency::Ms(ms)) = t.latency {
                            details.push(format!("{ms} ms"));
                        }
                        ui.label(RichText::new(details.join("  ·  ")).color(p.muted));
                    }
                    None => {
                        ui.label(RichText::new("Сервер не выбран").size(20.0).strong());
                        ui.label(
                            RichText::new("Выберите сервер в списке ниже или добавьте ссылку")
                                .color(p.muted),
                        );
                    }
                }
            });

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (label, fill) = if connected.is_some() {
                    ("Отключить", p.bad)
                } else {
                    ("Подключить", p.accent)
                };
                let enabled = !app.connecting && (connected.is_some() || target.is_some());
                let hint = if connected.is_some() {
                    "Остановить подключение"
                } else {
                    "Подключиться к выбранному серверу (Enter или двойной клик по строке)"
                };
                if theme::big_button(ui, label, fill, enabled)
                    .on_hover_text(hint)
                    .clicked()
                {
                    app.toggle_connection();
                }
                if app.connecting {
                    ui.spinner();
                }
            });
        });

        ui.add_space(6.0);
        ui.separator();
        ui.add_space(2.0);

        ui.horizontal(|ui| {
            ui.label(RichText::new("Режим").color(p.muted));
            mode_switch(app, ui);
            ui.add_space(12.0);
            ui.label(RichText::new("Ядро").color(p.muted));
            core_combo(app, ui);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    RichText::new(format!("{}:{}", app.settings.listen, app.settings.port))
                        .color(p.muted),
                )
                .on_hover_text("Локальный порт HTTP + SOCKS5");
            });
        });
    });
}

/// Segmented control: three mutually exclusive modes.
fn mode_switch(app: &mut RustBoxApp, ui: &mut egui::Ui) {
    let current = app.mode();
    let p = Palette::of(ui);
    Frame::new()
        .fill(ui.visuals().extreme_bg_color)
        .stroke(egui::Stroke::new(1.0, p.border))
        .corner_radius(8)
        .inner_margin(Margin::same(2))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.horizontal(|ui| {
                for mode in Mode::ALL {
                    let selected = mode == current;
                    let text = if selected {
                        RichText::new(mode.label()).color(p.on_accent)
                    } else {
                        RichText::new(mode.label())
                    };
                    let mut button = egui::Button::new(text).frame_when_inactive(selected);
                    if selected {
                        button = button.fill(p.accent);
                    }
                    if ui.add(button).on_hover_text(mode.hint()).clicked() {
                        app.set_mode(mode);
                    }
                }
            });
        });
}

fn core_combo(app: &mut RustBoxApp, ui: &mut egui::Ui) {
    let mut core = app.settings.core;
    egui::ComboBox::from_id_salt("core")
        .selected_text(core.display_name())
        .width(100.0)
        .show_ui(ui, |ui| {
            for (kind, version) in app.core_versions.clone() {
                let hover = match &version {
                    Ok(v) => v.clone(),
                    Err(e) => e.clone(),
                };
                let text = if version.is_ok() {
                    RichText::new(kind.display_name())
                } else {
                    RichText::new(format!("{} (не найдено)", kind.display_name())).weak()
                };
                ui.selectable_value(&mut core, kind, text)
                    .on_hover_text(hover);
            }
        });
    if core != app.settings.core {
        app.set_core(core);
    }
}

// ── Toolbar & table ───────────────────────────────────────────

fn toolbar(app: &mut RustBoxApp, ui: &mut egui::Ui) {
    let p = Palette::of(ui);
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut app.filter)
                .hint_text("🔍  Поиск по имени, адресу, протоколу")
                .desired_width(280.0),
        );
        if !app.filter.is_empty() && ui.button("×").on_hover_text("Сбросить поиск").clicked()
        {
            app.filter.clear();
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.menu_button("+  Добавить", |ui| add_menu_items(app, ui));

            if let Some(test) = &app.test {
                let done = test.total - test.pending.len();
                let (total, ok) = (test.total, test.ok);
                if ui
                    .button(RichText::new("■ Стоп").color(p.bad))
                    .on_hover_text("Остановить проверку")
                    .clicked()
                {
                    app.stop_test();
                } else {
                    ui.add(
                        egui::ProgressBar::new(done as f32 / total.max(1) as f32)
                            .desired_width(170.0)
                            .text(format!("{done}/{total} · работают {ok}")),
                    );
                }
            } else {
                // Right-to-left layout: the button first, the kind picker to its left.
                let scope = if app.selection.len() > 1 {
                    "выбранные серверы"
                } else {
                    "все серверы группы"
                };
                if ui
                    .button("Проверить")
                    .on_hover_text(format!(
                        "{}: {scope}",
                        test_kind_label(app.settings.test_kind)
                    ))
                    .clicked()
                {
                    app.run_test();
                }
                test_kind_combo(app, ui);
            }
        });
    });
}

fn test_kind_label(kind: TestKind) -> &'static str {
    match kind {
        TestKind::Url => "URL-тест",
        TestKind::Tcp => "TCP ping",
    }
}

fn test_kind_combo(app: &mut RustBoxApp, ui: &mut egui::Ui) {
    let mut kind = app.settings.test_kind;
    egui::ComboBox::from_id_salt("test_kind")
        .selected_text(test_kind_label(kind))
        .width(100.0)
        .show_ui(ui, |ui| {
            for k in TestKind::ALL {
                let hint = match k {
                    TestKind::Url => "Реальный запрос через сервер — показывает, работает ли он",
                    TestKind::Tcp => "Только доступность порта — быстро, но не гарантирует работу",
                };
                ui.selectable_value(&mut kind, k, test_kind_label(k))
                    .on_hover_text(hint);
            }
        });
    app.set_test_kind(kind);
}

fn profiles_table(app: &mut RustBoxApp, ui: &mut egui::Ui) {
    let p = Palette::of(ui);
    let profiles: Vec<Profile> = app.visible_profiles().into_iter().cloned().collect();
    if profiles.is_empty() {
        empty_state(app, ui);
        return;
    }

    let connected = app.connected_profile();
    let mut clicked: Option<(ProfileId, egui::Modifiers)> = None;
    let mut double_clicked = None;
    let modifiers = ui.input(|i| i.modifiers);

    theme::card(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.set_min_height(ui.available_height());
        TableBuilder::new(ui)
            .id_salt(("profiles", app.current_group))
            .striped(true)
            .auto_shrink([false, false])
            .sense(Sense::click())
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::exact(18.0))
            .column(Column::remainder().at_least(140.0).clip(true))
            .column(Column::initial(150.0).at_least(90.0).clip(true))
            .column(Column::initial(210.0).at_least(100.0).clip(true))
            .column(Column::exact(90.0))
            .header(26.0, |mut header| {
                header.col(|_| {});
                for (title, key) in [
                    ("Имя", SortKey::Name),
                    ("Протокол", SortKey::Type),
                    ("Адрес", SortKey::Address),
                    ("Задержка", SortKey::Latency),
                ] {
                    header.col(|ui| {
                        let arrow = match app.sort {
                            Some((k, true)) if k == key => "  ⏶",
                            Some((k, false)) if k == key => "  ⏷",
                            _ => "",
                        };
                        let resp = ui
                            .add(
                                egui::Label::new(
                                    RichText::new(format!("{title}{arrow}"))
                                        .size(12.0)
                                        .color(p.muted)
                                        .strong(),
                                )
                                .sense(Sense::click()),
                            )
                            .on_hover_text(
                                "Сортировать (ещё раз — обратный порядок, третий — сброс)",
                            );
                        if resp.clicked() {
                            app.toggle_sort(key);
                        }
                    });
                }
            })
            .body(|body| {
                body.rows(32.0, profiles.len(), |mut row| {
                    let pr = &profiles[row.index()];
                    let is_connected = connected == Some(pr.id);
                    row.set_selected(app.selection.contains(&pr.id));
                    row.col(|ui| {
                        if is_connected {
                            theme::dot(ui, p.ok, 4.0);
                        }
                    });
                    row.col(|ui| {
                        let name = RichText::new(pr.display_name());
                        ui.label(if is_connected { name.strong() } else { name });
                    });
                    row.col(|ui| {
                        ui.label(RichText::new(pr.type_label()).color(p.muted));
                    });
                    row.col(|ui| {
                        ui.label(RichText::new(pr.address()).color(p.muted));
                    });
                    let pending = app
                        .test
                        .as_ref()
                        .is_some_and(|t| t.pending.contains(&pr.id));
                    row.col(|ui| {
                        if pending {
                            ui.add(egui::Spinner::new().size(12.0));
                        } else {
                            latency_pill(ui, pr.latency.as_ref());
                        }
                    });

                    let resp = row.response();
                    if resp.double_clicked() {
                        double_clicked = Some(pr.id);
                    } else if resp.clicked() {
                        clicked = Some((pr.id, modifiers));
                    }
                    resp.context_menu(|ui| {
                        if !app.selection.contains(&pr.id) {
                            app.selection = [pr.id].into();
                        }
                        row_menu(app, ui, pr);
                    });
                });
            });
    });

    if let Some((id, m)) = clicked {
        if m.command {
            if !app.selection.remove(&id) {
                app.selection.insert(id);
            }
        } else if m.shift {
            let anchor = app.selection.iter().next().copied();
            let pos = |x: ProfileId| profiles.iter().position(|p| p.id == x);
            if let (Some(a), Some(b)) = (anchor.and_then(pos), pos(id)) {
                let (lo, hi) = (a.min(b), a.max(b));
                app.selection = profiles[lo..=hi].iter().map(|p| p.id).collect();
            }
        } else {
            app.selection = [id].into();
        }
    }
    if let Some(id) = double_clicked {
        app.selection = [id].into();
        app.connect(id);
    }
}

fn empty_state(app: &mut RustBoxApp, ui: &mut egui::Ui) {
    let p = Palette::of(ui);
    theme::card(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.set_min_height(ui.available_height());
        ui.vertical_centered(|ui| {
            ui.add_space((ui.available_height() * 0.28).max(12.0));
            if !app.filter.is_empty() {
                ui.label(RichText::new("Ничего не найдено").size(18.0).strong());
                ui.label(
                    RichText::new(format!("По запросу «{}» профилей нет", app.filter))
                        .color(p.muted),
                );
                ui.add_space(10.0);
                if ui.button("Сбросить поиск").clicked() {
                    app.filter.clear();
                }
                return;
            }
            ui.label(RichText::new("🌐").size(40.0));
            ui.add_space(4.0);
            ui.label(RichText::new("Добавьте сервер").size(20.0).strong());
            ui.label(
                RichText::new("Скопируйте ссылку на сервер или подписку и вставьте её сюда")
                    .color(p.muted),
            );
            ui.add_space(14.0);
            if theme::primary_button(ui, "Вставить из буфера  (Ctrl+V)").clicked() {
                app.import_clipboard();
            }
            ui.add_space(4.0);
            if ui.button("Добавить подписку по URL").clicked() {
                app.dialogs.subscription = Some(SubscriptionDialog::default());
            }
            ui.add_space(14.0);
            ui.label(
                RichText::new(
                    "vless:// · vmess:// · trojan:// · ss:// · hy2:// · tuic:// · socks://",
                )
                .small()
                .color(p.muted),
            );
        });
    });
}

fn row_menu(app: &mut RustBoxApp, ui: &mut egui::Ui, p: &Profile) {
    let selected: Vec<Profile> = app
        .store
        .profiles
        .iter()
        .filter(|x| app.selection.contains(&x.id))
        .cloned()
        .collect();
    if ui
        .add(egui::Button::new("▶  Подключить").shortcut_text("Enter"))
        .clicked()
    {
        app.connect(p.id);
        ui.close();
    }
    ui.separator();
    if ui.button("URL-тест").clicked() {
        app.url_test(selected.clone());
        ui.close();
    }
    if ui.button("TCP ping").clicked() {
        app.tcp_ping(selected.clone());
        ui.close();
    }
    ui.separator();
    if ui.button("Редактировать…").clicked() {
        app.dialogs.edit = Some(EditDialog::for_profile(p));
        ui.close();
    }
    if ui.button("Дублировать").clicked() {
        if let Some(id) = app.store.duplicate_profile(p.id) {
            app.selection = [id].into();
            app.save_store();
        }
        ui.close();
    }
    let links = if selected.len() > 1 {
        "Скопировать ссылки"
    } else {
        "Скопировать ссылку"
    };
    if ui.button(links).clicked() {
        app.copy_links(ui.ctx(), &selected);
        ui.close();
    }
    let json = app.settings.core.build_config(p, &(&app.settings).into());
    if ui
        .add_enabled(json.is_ok(), egui::Button::new("Скопировать конфиг ядра"))
        .clicked()
    {
        if let Ok(v) = json {
            ui.ctx()
                .copy_text(serde_json::to_string_pretty(&v).unwrap_or_default());
        }
        ui.close();
    }
    ui.separator();
    let delete = if selected.len() > 1 {
        format!("Удалить ({})…", selected.len())
    } else {
        "Удалить…".to_string()
    };
    if ui
        .add(
            egui::Button::new(RichText::new(delete).color(Palette::of(ui).bad))
                .shortcut_text("Del"),
        )
        .clicked()
    {
        app.dialogs.confirm_delete = Some(selected.iter().map(|p| p.id).collect());
        ui.close();
    }
}

fn latency_pill(ui: &mut egui::Ui, latency: Option<&Latency>) {
    let p = Palette::of(ui);
    match latency {
        None => {
            ui.label(RichText::new("—").color(p.muted));
        }
        Some(Latency::Ms(ms)) => {
            let color = match ms {
                0..=200 => p.ok,
                201..=500 => p.warn,
                _ => p.bad,
            };
            theme::pill(ui, format!("{ms} ms"), color);
        }
        Some(Latency::Error(e)) => {
            theme::pill(ui, "ошибка", p.bad).on_hover_text(e);
        }
    }
}

// ── Log & status bar ──────────────────────────────────────────

fn log_panel(app: &mut RustBoxApp, ui: &mut egui::Ui) {
    let p = Palette::of(ui);
    ui.horizontal(|ui| {
        theme::section_title(ui, "Лог");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add(egui::Button::new("×").frame_when_inactive(false))
                .on_hover_text("Скрыть лог")
                .clicked()
            {
                app.toggle_logs();
            }
            if ui.small_button("Очистить").clicked() {
                app.logs.clear();
            }
            if ui.small_button("Копировать").clicked() {
                ui.ctx().copy_text(app.logs.snapshot().join("\n"));
            }
        });
    });
    let lines = app.logs.snapshot();
    Frame::new()
        .fill(ui.visuals().extreme_bg_color)
        .corner_radius(6)
        .inner_margin(Margin::same(8))
        .show(ui, |ui| {
            egui::ScrollArea::both()
                .id_salt("log_scroll")
                .stick_to_bottom(true)
                .auto_shrink([false, false])
                .show_rows(ui, 16.0, lines.len(), |ui, range| {
                    for line in &lines[range] {
                        let color = if line.contains("ERROR")
                            || line.contains("FATAL")
                            || line.contains("[Error]")
                        {
                            p.bad
                        } else if line.contains("WARN") || line.contains("[Warning]") {
                            p.warn
                        } else {
                            p.muted
                        };
                        ui.label(RichText::new(line).monospace().color(color));
                    }
                });
        });
}

fn status_bar(app: &mut RustBoxApp, ui: &mut egui::Ui) {
    let p = Palette::of(ui);
    ui.horizontal(|ui| {
        if let Some(status) = &app.status
            && status.at.elapsed().as_secs() < 15
        {
            let (icon, color) = if status.error {
                ("⚠", p.bad)
            } else {
                ("✓", p.muted)
            };
            ui.add(
                egui::Label::new(RichText::new(format!("{icon}  {}", status.text)).color(color))
                    .truncate(),
            )
            .on_hover_text(&status.text);
        } else {
            ui.label(RichText::new(" ").small());
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let label = if app.unseen_errors > 0 && !app.show_logs {
                RichText::new(format!("Лог  ● {}", app.unseen_errors)).color(p.bad)
            } else {
                RichText::new("Лог")
            };
            if ui
                .add(egui::Button::selectable(app.show_logs, label))
                .on_hover_text("Показать или скрыть лог ядра")
                .clicked()
            {
                app.toggle_logs();
            }
        });
    });
}

// ── Formatting ────────────────────────────────────────────────

fn human_bytes(b: u64) -> String {
    const UNITS: [&str; 5] = ["Б", "КБ", "МБ", "ГБ", "ТБ"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{b} Б")
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}

fn human_minutes(m: u64) -> String {
    match m {
        0 => "только что".into(),
        1..=59 => format!("{m} мин назад"),
        60..=1439 => format!("{} ч назад", m / 60),
        _ => format!("{} дн назад", m / 1440),
    }
}

/// "1 день", "3 дня", "17 дней".
fn days_word(n: i64) -> String {
    let word = match (n % 10, n % 100) {
        (1, r) if r != 11 => "день",
        (2..=4, r) if !(12..=14).contains(&r) => "дня",
        _ => "дней",
    };
    format!("{n} {word}")
}

fn format_uptime(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, secs % 3600 / 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn russian_day_forms() {
        let cases = [
            (1, "1 день"),
            (2, "2 дня"),
            (5, "5 дней"),
            (11, "11 дней"),
            (12, "12 дней"),
            (21, "21 день"),
            (24, "24 дня"),
            (111, "111 дней"),
        ];
        for (n, want) in cases {
            assert_eq!(days_word(n), want);
        }
    }
}

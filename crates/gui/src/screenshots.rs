//! Скриншоты окна без реального экрана (egui_kittest + wgpu): для ревью
//! дизайна и сравнения «до/после».
//!
//! ```sh
//! scripts/screenshots.sh          # PNG в target/screenshots/
//! ```
//!
//! Тест помечен `#[ignore]`, поэтому обычный `cargo test` его не запускает.
//! Настройки и профили берутся из XDG-каталогов, которые скрипт подменяет на
//! временные: реальные данные пользователя не читаются и не меняются.

use std::path::PathBuf;

use eframe::egui;
use egui_kittest::Harness;
use rustbox_core::model::{Latency, Subscription, SubscriptionInfo};
use rustbox_core::{link, storage::DEFAULT_GROUP};

use crate::app::{RustBoxApp, unix_now};

const DEMO_LINKS: &[&str] = &[
    "vless://11111111-1111-1111-1111-111111111111@se1.example.net:443?encryption=none&flow=xtls-rprx-vision&security=reality&sni=www.example.com&fp=chrome&pbk=Kx16VbBQ81OS70loTk0TdpZazBhFsjzbmooJNX_SRyU&sid=abcd&type=tcp#Стокгольм",
    "vless://11111111-1111-1111-1111-111111111111@ru1.example.net:443?encryption=none&flow=xtls-rprx-vision&security=reality&sni=ya.example.ru&fp=chrome&pbk=Kx16VbBQ81OS70loTk0TdpZazBhFsjzbmooJNX_SRyU&sid=abcd&type=tcp#Москва → Стокгольм",
    "vless://22222222-2222-2222-2222-222222222222@nl.example.net:443?encryption=none&security=tls&sni=nl.example.net&type=ws&path=%2Fws#Нидерланды WS",
    "trojan://secret@de.example.net:443?sni=de.example.net#Германия Trojan",
    "hysteria2://pass@fi.example.net:8443?sni=fi.example.net#Финляндия Hysteria2",
    "ss://MjAyMi1ibGFrZTMtYWVzLTI1Ni1nY206WkdWdGIzQmhjM04zYjNKaw==@pl.example.net:8388#Польша SS-2022",
    "tuic://33333333-3333-3333-3333-333333333333:pass@lv.example.net:443?sni=lv.example.net&congestion_control=bbr#Латвия TUIC",
    "vmess://eyJ2IjoiMiIsInBzIjoiVVNBIFZNZXNzIiwiYWRkIjoidXMuZXhhbXBsZS5uZXQiLCJwb3J0IjoiNDQzIiwiaWQiOiI0NDQ0NDQ0NC00NDQ0LTQ0NDQtNDQ0NC00NDQ0NDQ0NDQ0NDQiLCJhaWQiOiIwIiwibmV0Ijoid3MiLCJ0eXBlIjoibm9uZSIsImhvc3QiOiJ1cy5leGFtcGxlLm5ldCIsInBhdGgiOiIvIiwidGxzIjoidGxzIn0=",
];

fn out_dir() -> PathBuf {
    std::env::var_os("RUSTBOX_SCREENSHOT_DIR")
        .map(PathBuf::from)
        .expect("запускай через scripts/screenshots.sh (нужен RUSTBOX_SCREENSHOT_DIR)")
}

fn harness(theme: egui::Theme) -> Harness<'static, RustBoxApp> {
    Harness::builder()
        .with_size(egui::vec2(1100.0, 700.0))
        .with_theme(theme)
        .wgpu()
        .build_eframe(|cc| RustBoxApp::new(cc).expect("приложение не создалось"))
}

/// Типичное состояние: подписка с трафиком, пачка профилей, часть проверена.
fn fill_demo(app: &mut RustBoxApp) {
    let profiles: Vec<_> = DEMO_LINKS
        .iter()
        .filter_map(|l| link::parse_link(l).ok())
        .collect();
    assert_eq!(
        profiles.len(),
        DEMO_LINKS.len(),
        "демо-ссылка не разобралась"
    );

    let sub = app.store.add_group(
        "Мой VPN",
        Some(Subscription {
            url: "https://sub.example.net/sub/token".into(),
            last_updated: Some(unix_now() - 25 * 60),
            info: Some(SubscriptionInfo {
                upload: 1_200_000_000,
                download: 38_500_000_000,
                total: 100_000_000_000,
                expire: unix_now() + 17 * 86400,
            }),
            ..Default::default()
        }),
    );
    let ids = app.store.add_profiles(sub, profiles.clone());
    app.store
        .add_profiles(DEFAULT_GROUP, profiles[2..5].to_vec());
    app.store.add_group("Резерв", None);

    let latencies = [
        Some(Latency::Ms(48)),
        Some(Latency::Ms(96)),
        Some(Latency::Ms(312)),
        Some(Latency::Error("timeout".into())),
        Some(Latency::Ms(640)),
        None,
        Some(Latency::Ms(143)),
        Some(Latency::Error("connection refused".into())),
    ];
    for (id, l) in ids.iter().zip(latencies) {
        if let Some(l) = l {
            app.store.set_latency(*id, l);
        }
    }
    app.current_group = sub;
    app.selection = [ids[0]].into();
}

fn save(h: &mut Harness<'_, RustBoxApp>, name: &str) {
    // Фиксированное число кадров: спиннеры перерисовываются бесконечно
    h.run_steps(4);
    let image = h.render().expect("рендер не удался");
    let path = out_dir().join(format!("{name}.png"));
    image.save(&path).expect("не удалось сохранить PNG");
    eprintln!("  {}", path.display());
}

#[test]
#[ignore = "скриншоты: scripts/screenshots.sh"]
fn screenshots() {
    for (theme, suffix) in [(egui::Theme::Dark, "dark"), (egui::Theme::Light, "light")] {
        let mut h = harness(theme);
        save(&mut h, &format!("empty-{suffix}"));

        fill_demo(h.state_mut());
        save(&mut h, &format!("main-{suffix}"));

        h.state_mut().connecting = true;
        save(&mut h, &format!("connecting-{suffix}"));
        h.state_mut().connecting = false;

        let settings = h.state().settings.clone();
        h.state_mut().dialogs.settings = Some(settings);
        save(&mut h, &format!("settings-{suffix}"));
        h.state_mut().dialogs.settings = None;

        h.state_mut().dialogs.subscription = Some(Default::default());
        save(&mut h, &format!("subscription-{suffix}"));
        h.state_mut().dialogs.subscription = None;

        let ids: Vec<_> = h
            .state()
            .visible_profiles()
            .iter()
            .take(3)
            .map(|p| p.id)
            .collect();
        h.state_mut().dialogs.confirm_delete = Some(ids);
        save(&mut h, &format!("confirm-{suffix}"));
        h.state_mut().dialogs.confirm_delete = None;

        h.state_mut().show_logs = true;
        h.state_mut()
            .error("ядро неожиданно завершилось, смотрите лог");
        save(&mut h, &format!("logs-{suffix}"));
    }
}

//! Native macOS menus and a crisp, theme-aware status-bar mark.
use crate::{i18n, App};
use tray_icon::{
    menu::{
        accelerator::{Accelerator, Code, Modifiers},
        Menu, MenuItem, PredefinedMenuItem, Submenu,
    },
    Icon,
};

pub fn is_active() -> bool {
    objc2::MainThreadMarker::new()
        .is_some_and(|mtm| objc2_app_kit::NSApplication::sharedApplication(mtm).isActive())
}

pub fn hide() {
    if let Some(mtm) = objc2::MainThreadMarker::new() {
        objc2_app_kit::NSApplication::sharedApplication(mtm).hide(None);
    }
}

fn login_agent_path() -> std::io::Result<std::path::PathBuf> {
    let home = std::env::var_os("HOME")
        .ok_or_else(|| std::io::Error::other("Не найдена домашняя папка"))?;
    Ok(std::path::PathBuf::from(home).join("Library/LaunchAgents/ru.everty.multilink.plist"))
}

fn login_agent() -> std::io::Result<String> {
    let executable = std::env::current_exe()?.to_string_lossy().into_owned();
    let executable = executable
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;");
    Ok(format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>ru.everty.multilink</string>
<key>ProgramArguments</key><array><string>{executable}</string><string>--start-hidden</string></array>
<key>RunAtLoad</key><true/>
<key>LimitLoadToSessionType</key><string>Aqua</string>
</dict></plist>
"#
    ))
}

pub fn autostart_enabled() -> std::io::Result<bool> {
    match std::fs::read_to_string(login_agent_path()?) {
        Ok(contents) => Ok(contents == login_agent()?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

pub fn set_autostart(enabled: bool) -> std::io::Result<()> {
    let path = login_agent_path()?;
    if enabled {
        std::fs::create_dir_all(path.parent().unwrap())?;
        std::fs::write(path, login_agent()?)
    } else if path.exists() {
        std::fs::remove_file(path)
    } else {
        Ok(())
    }
}

pub fn status_icon(connected: bool) -> Result<Icon, tray_icon::BadIcon> {
    // Render at 4x, downsample to a Retina bitmap. macOS uses the alpha channel
    // as a template, choosing the appropriate colour for the menu bar itself.
    let mut image = image::RgbaImage::new(88, 88);
    for (x, y, pixel) in image.enumerate_pixels_mut() {
        let x = (x as f32 + 0.5) / 4.0;
        let y = (y as f32 + 0.5) / 4.0;
        let rounded = |inset: f32, radius: f32| {
            let qx = (x - 10.5).abs() - (8.0 - inset - radius);
            let qy = (y - 10.5).abs() - (8.0 - inset - radius);
            qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) <= radius
        };
        let border = rounded(0.0, 4.5) && !rounded(1.5, 3.0);
        let ex = x + (y - 10.5) * 0.16;
        let stem = (6.5..8.1).contains(&ex) && (6.5..14.5).contains(&y);
        let bars = (6.5..14.5).contains(&ex)
            && ((6.5..8.0).contains(&y) || (9.75..11.25).contains(&y) || (13.0..14.5).contains(&y));
        let dot = (x - 17.2).hypot(y - 17.2);
        let filled = if dot < 3.5 {
            if connected {
                dot < 2.1
            } else {
                (1.25..2.1).contains(&dot)
            }
        } else {
            border || stem || bars
        };
        *pixel = image::Rgba([0, 0, 0, if filled { 255 } else { 0 }]);
    }
    let image = image::imageops::resize(&image, 44, 44, image::imageops::FilterType::Lanczos3);
    Icon::from_rgba(image.into_raw(), 44, 44)
}

pub fn install_menu(app: &App) -> tray_icon::menu::Result<Menu> {
    let root = Menu::new();
    let language = app.language();
    let text = |s: &str| i18n::tr(language, s).into_owned();
    let application = Submenu::new("Everty MultiLink", true);
    let item = |id: &str, label: &'static str, key: Option<Code>| {
        let accelerator = key.map(|key| Accelerator::new(Some(Modifiers::SUPER), key));
        let item = MenuItem::with_id(id, text(label), true, accelerator);
        item
    };
    application.append_items(&[
        &item("about", "О программе", None),
        &PredefinedMenuItem::separator(),
        &item("settings", "Настройки…", Some(Code::Comma)),
        &PredefinedMenuItem::separator(),
        &PredefinedMenuItem::services(Some(&text("Службы"))),
        &PredefinedMenuItem::separator(),
        &PredefinedMenuItem::hide(Some(&text("Скрыть MultiLink"))),
        &PredefinedMenuItem::hide_others(Some(&text("Скрыть остальные"))),
        &PredefinedMenuItem::show_all(Some(&text("Показать все"))),
        &PredefinedMenuItem::separator(),
        &item("exit", "Завершить MultiLink", Some(Code::KeyQ)),
    ])?;
    let window = Submenu::new(text("Окно"), true);
    window.append_items(&[
        &item("open", "Открыть MultiLink", Some(Code::KeyO)),
        &item("hide-window", "Закрыть окно", Some(Code::KeyW)),
        &PredefinedMenuItem::minimize(Some(&text("Свернуть"))),
        &PredefinedMenuItem::separator(),
        &item("compact", "Компактное окно", None),
        &item("advanced", "Расширенное окно", None),
    ])?;
    root.append_items(&[&application, &window])?;
    window.set_as_windows_menu_for_nsapp();
    root.init_for_nsapp();
    Ok(root)
}

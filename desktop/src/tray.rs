//! The system tray: builds the menu from a [`MenuModel`] and afterwards
//! patches text, enabled and check states in place. The menu is rebuilt only
//! when its shape changes (groups, nodes or subscriptions added or removed).
use crate::{
    activation, controller,
    model::{Action, Entry, Icon, MenuModel},
    window,
};
use std::sync::Mutex;
use tauri::{
    AppHandle, Manager as _, Wry,
    image::Image,
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu},
    tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent},
};

enum Handle {
    Item(MenuItem<Wry>),
    Check(CheckMenuItem<Wry>),
    Submenu(Submenu<Wry>),
    Separator,
}

struct Current {
    tray: TrayIcon,
    handles: Vec<Handle>,
    model: MenuModel,
    actions: Vec<Option<Action>>,
}

#[derive(Default)]
pub struct TrayState(Mutex<Option<Current>>);

fn icon(kind: Icon) -> Image<'static> {
    let bytes: &'static [u8] = match kind {
        Icon::Normal => include_bytes!("../icons/tray-normal.png"),
        Icon::Tun => include_bytes!("../icons/tray-tun.png"),
        Icon::Offline => include_bytes!("../icons/tray-offline.png"),
    };
    Image::from_bytes(bytes).expect("bundled tray icon is a valid PNG")
}

/// Menu ids are positions in depth-first order: never parsed from labels.
fn id(index: usize) -> String {
    format!("m{index}")
}

fn build_into(
    app: &AppHandle,
    entries: &[Entry],
    handles: &mut Vec<Handle>,
    append: &mut dyn FnMut(&dyn tauri::menu::IsMenuItem<Wry>) -> tauri::Result<()>,
) -> tauri::Result<()> {
    for entry in entries {
        let index = handles.len();
        match entry {
            Entry::Item { label, enabled, .. } => {
                let item = MenuItem::with_id(app, id(index), label, *enabled, None::<&str>)?;
                append(&item)?;
                handles.push(Handle::Item(item));
            }
            Entry::Check {
                label,
                enabled,
                checked,
                ..
            } => {
                let item = CheckMenuItem::with_id(app, id(index), label, *enabled, *checked, None::<&str>)?;
                append(&item)?;
                handles.push(Handle::Check(item));
            }
            Entry::Submenu {
                label,
                enabled,
                children,
            } => {
                let submenu = Submenu::with_id(app, id(index), label, *enabled)?;
                handles.push(Handle::Submenu(submenu.clone()));
                build_into(app, children, handles, &mut |child| submenu.append(child))?;
                append(&submenu)?;
            }
            Entry::Separator => {
                append(&PredefinedMenuItem::separator(app)?)?;
                handles.push(Handle::Separator);
            }
        }
    }
    Ok(())
}

fn build(app: &AppHandle, model: &MenuModel) -> tauri::Result<(Menu<Wry>, Vec<Handle>)> {
    let menu = Menu::new(app)?;
    let mut handles = Vec::new();
    build_into(app, &model.entries, &mut handles, &mut |item| menu.append(item))?;
    Ok((menu, handles))
}

fn patch(old: &Entry, new: &Entry, handle: &Handle) -> tauri::Result<()> {
    match (old, new, handle) {
        (
            Entry::Item {
                label: old_label,
                enabled: old_enabled,
                ..
            },
            Entry::Item { label, enabled, .. },
            Handle::Item(item),
        ) => {
            if old_label != label {
                item.set_text(label)?;
            }
            if old_enabled != enabled {
                item.set_enabled(*enabled)?;
            }
        }
        (
            Entry::Check {
                label: old_label,
                enabled: old_enabled,
                ..
            },
            Entry::Check { label, enabled, .. },
            Handle::Check(item),
        ) => {
            if old_label != label {
                item.set_text(label)?;
            }
            if old_enabled != enabled {
                item.set_enabled(*enabled)?;
            }
        }
        (
            Entry::Submenu {
                label: old_label,
                enabled: old_enabled,
                ..
            },
            Entry::Submenu { label, enabled, .. },
            Handle::Submenu(submenu),
        ) => {
            if old_label != label {
                submenu.set_text(label)?;
            }
            if old_enabled != enabled {
                submenu.set_enabled(*enabled)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Check items toggle themselves when clicked; the model stays the truth.
fn sync_checks(current: &Current) -> tauri::Result<()> {
    for (entry, handle) in current.model.flatten().into_iter().zip(&current.handles) {
        if let (Entry::Check { checked, .. }, Handle::Check(item)) = (entry, handle)
            && item.is_checked()? != *checked
        {
            item.set_checked(*checked)?;
        }
    }
    Ok(())
}

pub fn create(app: &AppHandle, model: MenuModel) -> tauri::Result<()> {
    let (menu, handles) = build(app, &model)?;
    let tray = TrayIconBuilder::with_id("main")
        .icon(icon(model.icon))
        // The StatusNotifierItem title heads the tooltip; the model fills its body.
        .title("Mihomo Server")
        .tooltip(&model.tooltip)
        .menu(&menu)
        // Left click opens the management window; the menu is on right click.
        .show_menu_on_left_click(false)
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                activation::adopt_tray_token();
                window::open_preferred(tray.app_handle());
            }
        })
        .on_menu_event(|app, event| on_menu_event(app, event.id().as_ref()))
        .build(app)?;
    let actions = model.actions();
    *lock(app) = Some(Current {
        tray,
        handles,
        model,
        actions,
    });
    Ok(())
}

fn lock(app: &AppHandle) -> std::sync::MutexGuard<'_, Option<Current>> {
    app.state::<TrayState>()
        .inner()
        .0
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn apply_now(app: &AppHandle, model: MenuModel) -> tauri::Result<()> {
    let mut guard = lock(app);
    let Some(current) = guard.as_mut() else {
        return Ok(());
    };
    if current.model != model {
        if current.model.shape() == model.shape() {
            for ((old, new), handle) in current
                .model
                .flatten()
                .into_iter()
                .zip(model.flatten())
                .zip(&current.handles)
            {
                patch(old, new, handle)?;
            }
        } else {
            let (menu, handles) = build(app, &model)?;
            current.tray.set_menu(Some(menu))?;
            current.handles = handles;
        }
        if current.model.icon != model.icon {
            current.tray.set_icon(Some(icon(model.icon)))?;
        }
        if current.model.tooltip != model.tooltip {
            current.tray.set_tooltip(Some(&model.tooltip))?;
        }
        current.actions = model.actions();
        current.model = model;
    }
    sync_checks(current)
}

/// Apply a model on the main thread, where menus live.
pub fn apply(app: &AppHandle, model: MenuModel) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Err(error) = apply_now(&handle, model) {
            eprintln!("tray update failed: {error}");
        }
    });
}

fn on_menu_event(app: &AppHandle, id: &str) {
    let action = id
        .strip_prefix('m')
        .and_then(|index| index.parse::<usize>().ok())
        .and_then(|index| lock(app).as_ref()?.actions.get(index).cloned().flatten());
    if let Some(action) = action {
        controller::dispatch(app, action);
    }
    if let Some(current) = lock(app).as_ref() {
        let _ = sync_checks(current);
    }
}

//! Windows notification-area icon: Open, Support the author, Exit. Lives on the message-loop thread of the tray
//! process; `update` is called from the loop's timer.

use crate::strings;
use crate::support;
pub use crate::tray_common::TrayState;
use crate::tray_common::{perform, Action};
use std::cell::RefCell;
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

struct Tray {
    icon: TrayIcon,
    open: MenuItem,
    support: MenuItem,
    exit: MenuItem,
    last: TrayState,
}

thread_local! {
    static TRAY: RefCell<Option<Tray>> = const { RefCell::new(None) };
}

/// Creates the icon. Call on the message-loop thread.
pub fn create(state: &TrayState, icon_png: &[u8]) -> bool {
    let Ok(data) = eframe::icon_data::from_png_bytes(icon_png) else { return false };
    let Ok(icon_img) = Icon::from_rgba(data.rgba, data.width, data.height) else { return false };
    let t = strings::TABLE.get(state.lang);
    let open = MenuItem::new(t.tray_open, true, None);
    let support_item = MenuItem::new(support::tray_label(state.lang), true, None);
    let exit = MenuItem::new(t.tray_exit, true, None);
    let menu = Menu::new();
    if menu.append_items(&[&open, &PredefinedMenuItem::separator(), &support_item, &exit]).is_err() {
        return false;
    }
    let Ok(icon) = TrayIconBuilder::new().with_icon(icon_img).with_tooltip(&state.tooltip).with_menu(Box::new(menu)).with_menu_on_left_click(false).build() else {
        return false;
    };
    let ids = (open.id().clone(), support_item.id().clone(), exit.id().clone());
    MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
        let a = if e.id == ids.0 {
            Action::Open
        } else if e.id == ids.1 {
            Action::Support
        } else if e.id == ids.2 {
            Action::Exit
        } else {
            return;
        };
        perform(a);
    }));
    TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
        if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
            perform(Action::Open);
        }
    }));
    TRAY.with(|t| *t.borrow_mut() = Some(Tray { icon, open, support: support_item, exit, last: state.clone() }));
    true
}

/// Draws a new state (tooltip and, when the language changed, the menu words). Call on the loop thread.
pub fn update(state: TrayState) {
    TRAY.with(|t| {
        let mut b = t.borrow_mut();
        let Some(tr) = b.as_mut() else { return };
        if tr.last == state {
            return;
        }
        if tr.last.lang != state.lang {
            let t = strings::TABLE.get(state.lang);
            tr.open.set_text(t.tray_open);
            tr.support.set_text(support::tray_label(state.lang));
            tr.exit.set_text(t.tray_exit);
        }
        let _ = tr.icon.set_tooltip(Some(&state.tooltip));
        tr.last = state;
    });
}

pub fn remove() {
    TRAY.with(|t| {
        if let Some(tr) = t.borrow().as_ref() {
            let _ = tr.icon.set_visible(false);
        }
        *t.borrow_mut() = None;
    });
}

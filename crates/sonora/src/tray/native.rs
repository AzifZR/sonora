use tokio::sync::mpsc::UnboundedSender;
use tray_icon::menu::{
    Icon as MenuIcon, IconMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem,
};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use super::{Art, Event, Shown};

const TOOLTIP: &str = "Sonora";
const PNG: &[u8] = match cfg!(target_os = "macos") {
    true => include_bytes!("../../../../assets/tray/template-64.png"),
    false => include_bytes!("../../../../assets/tray/sonora.png"),
};
const MENU_ON_CLICK: bool = cfg!(target_os = "macos");

pub struct Icon {
    icon: TrayIcon,
    caption: IconMenuItem,
    toggle: MenuItem,
    previous: MenuItem,
    next: MenuItem,
    show: MenuItem,
    quit: MenuItem,
}

impl Icon {
    pub fn new(sender: UnboundedSender<Event>) -> Option<Self> {
        let image = match image::load_from_memory(PNG) {
            Ok(image) => image.into_rgba8(),
            Err(error) => {
                log::warn!("tray: cannot decode the tray icon: {error:#}");
                return None;
            }
        };
        let (width, height) = image.dimensions();
        let icon = match tray_icon::Icon::from_rgba(image.into_raw(), width, height) {
            Ok(icon) => icon,
            Err(error) => {
                log::warn!("tray: cannot build the tray icon: {error:#}");
                return None;
            }
        };

        let caption = IconMenuItem::with_id("caption", "", false, None, None);
        let toggle = MenuItem::with_id("toggle", "", true, None);
        let previous = MenuItem::with_id("previous", "", true, None);
        let next = MenuItem::with_id("next", "", true, None);
        let show = MenuItem::with_id("show", "", true, None);
        let quit = MenuItem::with_id("quit", "", true, None);
        let menu = Menu::new();
        if let Err(error) = menu.append_items(&[
            &caption,
            &PredefinedMenuItem::separator(),
            &toggle,
            &previous,
            &next,
            &PredefinedMenuItem::separator(),
            &show,
            &quit,
        ]) {
            log::warn!("tray: cannot build the tray menu: {error:#}");
            return None;
        }

        let built = TrayIconBuilder::new()
            .with_icon(icon)
            .with_icon_as_template(true)
            .with_tooltip(TOOLTIP)
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(MENU_ON_CLICK)
            .build();
        let icon = match built {
            Ok(icon) => icon,
            Err(error) => {
                log::warn!("tray: cannot place the tray icon: {error:#}");
                return None;
            }
        };

        let menus = sender.clone();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            if let Some(event) = translate(&event.id) {
                menus.send(event).ok();
            }
        }));
        TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
            let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            else {
                return;
            };
            if !MENU_ON_CLICK {
                sender.send(Event::Show).ok();
            }
        }));

        Some(Self {
            icon,
            caption,
            toggle,
            previous,
            next,
            show,
            quit,
        })
    }

    pub fn show(&mut self, shown: &Shown) {
        // the status notifier hosts read the caption off the tooltip themselves; here it has to
        // be pushed, or hovering the icon only ever says Sonora
        if let Err(error) = self.icon.set_tooltip(Some(&shown.caption)) {
            log::warn!("tray: cannot set the tooltip: {error:#}");
        }
        self.caption.set_text(&shown.caption);
        self.caption.set_icon(cover(shown.artwork.as_ref()));
        self.caption.set_enabled(shown.song);
        self.toggle.set_text(&shown.toggle);
        self.previous.set_text(&shown.previous);
        self.next.set_text(&shown.next);
        self.show.set_text(&shown.show);
        self.quit.set_text(&shown.quit);
    }
}

/// The cover as a menu icon. A cover that cannot be built is simply left off the row.
fn cover(art: Option<&Art>) -> Option<MenuIcon> {
    let art = art?;
    match MenuIcon::from_rgba(art.data.clone(), art.width, art.height) {
        Ok(icon) => Some(icon),
        Err(error) => {
            log::warn!("tray: cannot build the cover icon: {error:#}");
            None
        }
    }
}

fn translate(id: &MenuId) -> Option<Event> {
    Some(match id.0.as_str() {
        "caption" => Event::Song,
        "toggle" => Event::Toggle,
        "previous" => Event::Previous,
        "next" => Event::Next,
        "show" => Event::Show,
        "quit" => Event::Quit,
        _ => return None,
    })
}

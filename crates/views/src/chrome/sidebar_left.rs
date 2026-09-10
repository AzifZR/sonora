use ui::{
    ActiveTheme as _, Button, Card, DraggedPin, Edge, Panel, Pin, Pinnable as _, Popup, SNUG,
    Scrollbar, Shield, Side, Tabs, Text, drop_gap, drop_marker,
};

use crate::shared::pins::Pinned as _;
use gpui::prelude::*;
use gpui::{
    AnyElement, App, Context, DragMoveEvent, ElementId, Entity, Hsla, ListAlignment, ListState,
    MouseButton, MouseDownEvent, Pixels, Point, Render, ScrollWheelEvent, list, svg,
};
use gpui::{Window, div, px};
use router::{
    Destination, LibraryTab, NavEntry, Navigation, NavigationEvent, SettingsTab, navigate,
};
use state::{
    AppSettings, Library, LibraryState, Origin, Playback, PlaybackState, Session, Shelf, Sonora,
};

use crate::shared::menus::{ItemMenu, item_menu, pin_menu};
use music::{LibraryItem, LibraryItemKind, LibraryOrder};

const NAV: [(Option<NavEntry>, &str, Destination); 6] = [
    (Some(NavEntry::Home), "icons/house.svg", Destination::Home),
    (
        Some(NavEntry::Search),
        "icons/search.svg",
        Destination::Search,
    ),
    (
        Some(NavEntry::Library),
        "icons/library-big.svg",
        Destination::Library(LibraryTab::Songs),
    ),
    (
        Some(NavEntry::Local),
        "icons/file-music.svg",
        Destination::Local(LibraryTab::Songs),
    ),
    (
        Some(NavEntry::History),
        "icons/rotate-ccw-clock.svg",
        Destination::History,
    ),
    (
        None,
        "icons/settings.svg",
        Destination::Settings(SettingsTab::General),
    ),
];

const LIBRARY_TABS: [(&str, LibraryTab); 4] = [
    ("nav-songs", LibraryTab::Songs),
    ("nav-albums", LibraryTab::Albums),
    ("nav-artists", LibraryTab::Artists),
    ("nav-playlists", LibraryTab::Playlists),
];

const SETTINGS_TABS: [(&str, SettingsTab); 5] = [
    ("settings-tab-general", SettingsTab::General),
    ("settings-tab-appearance", SettingsTab::Appearance),
    ("settings-tab-playback", SettingsTab::Playback),
    ("settings-tab-privacy", SettingsTab::Privacy),
    ("settings-tab-about", SettingsTab::About),
];

const MIN_WIDTH: Pixels = px(160.);
const MAX_WIDTH: Pixels = px(400.);
const HINT_HEIGHT: Pixels = px(42.);

pub(crate) struct SidebarLeft {
    settings: Entity<AppSettings>,
    session: Entity<Session>,
    trail: Entity<Navigation>,
    at: Destination,
    width: Pixels,
    open: bool,
    cramped: bool,
    forced: Option<bool>,
    library_open: bool,
    local_open: bool,
    settings_open: bool,
    dropping: bool,
    drop_gap: Option<usize>,
    playback: Entity<Playback>,
    track_menu: ItemMenu,
    context_menu: Option<(Pin, Point<Pixels>)>,
    scrollbar: Entity<Scrollbar>,
    scroll: ListState,
    row_height: Pixels,
    scroll_width: Option<Pixels>,
    library: Entity<Library>,
    library_context_menu: Option<(LibraryItem, Point<Pixels>)>,
    library_popovers: ui::Popovers,
}

impl SidebarLeft {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let settings = Sonora::global(cx).settings.clone();
        let session = Sonora::global(cx).session.clone();
        let playback = Sonora::global(cx).playback.clone();
        let library = Sonora::global(cx).library.clone();
        cx.observe(&library, |_, _, cx| cx.notify()).detach();
        cx.observe(&playback, |_, _, cx| cx.notify()).detach();
        let me = cx.entity_id();
        let playlist_scrollbar = cx.new(|_| Scrollbar::inset().watching(me));
        let scroll = ListState::new(1, ListAlignment::Top, cx.theme().metrics.list_row);
        let scrollbar = cx.new(|_| Scrollbar::list(scroll.clone()).watching(me));
        let width = px(settings.read(cx).sidebar_width()).clamp(MIN_WIDTH, MAX_WIDTH);
        let open = settings.read(cx).sidebar_open();
        let trail = router::trail(cx);

        cx.observe(&session, |_, _, cx| cx.notify()).detach();
        cx.observe(&settings, |_, _, cx| cx.notify()).detach();
        cx.observe(&trail, |_, _, cx| cx.notify()).detach();
        cx.subscribe(&trail, |this, _, _: &NavigationEvent, cx| {
            this.dismiss(cx);
            cx.notify();
        })
        .detach();

        let at = trail.read(cx).current();
        let library_open = matches!(at, Destination::Library(_));
        let local_open = matches!(at, Destination::Local(_));
        let settings_open = matches!(at, Destination::Settings(_));

        Self {
            settings,
            session,
            trail,
            at,
            width,
            open,
            forced: None,
            cramped: false,
            library_open,
            local_open,
            settings_open,
            dropping: false,
            drop_gap: None,
            playback,
            track_menu: ItemMenu::new(playlist_scrollbar),
            context_menu: None,
            scrollbar,
            scroll,
            row_height: cx.theme().metrics.list_row,
            scroll_width: None,
            library,
            library_context_menu: None,
            library_popovers: ui::Popovers::default(),
        }
    }

    fn follow(&mut self, current: &Destination) {
        if self.at == *current {
            return;
        }
        self.at = current.clone();
        let (library, local, settings) = expanded(current);
        self.library_open |= library;
        self.local_open |= local;
        self.settings_open |= settings;
    }

    fn dismiss_menu(&mut self, cx: &mut Context<Self>) {
        self.track_menu.reset(cx);
        self.context_menu = None;
        self.library_context_menu = None;
        cx.notify();
    }

    fn menu(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let (menu, position) = if let Some((item, position)) = &self.library_context_menu {
            let pin = item_pin(item);
            let mut menu = match &pin {
                Some(pin) if self.context_menu.is_some() => {
                    pin_menu(pin, &self.track_menu, self.playback.clone(), cx)
                }
                Some(pin) => item_menu(pin, &self.track_menu, self.playback.clone(), cx),
                None => ui::Menu::new("sidebar-library-context"),
            };
            let library = self.library.read(cx);
            if let Some(current) = library
                .sidebar_items()
                .and_then(|items| items.iter().find(|current| current.uri == item.uri))
            {
                let pinned = current.pinned;
                let pending = library.sidebar_pin_pending();
                let uri = current.uri.clone();
                let library = self.library.clone();
                if pin.is_some() {
                    menu = menu.item(ui::MenuItem::separator("library-pin-separator"));
                }
                menu = menu.item(
                    ui::MenuItem::new(
                        "library-pin",
                        i18n::lookup(
                            match (self.context_menu.is_some(), pinned) {
                                (true, true) => "nav-unpin-spotify",
                                (true, false) => "nav-pin-spotify",
                                (false, true) => "nav-unpin",
                                (false, false) => "nav-pin",
                            },
                            None,
                        ),
                    )
                    .icon("icons/pin.svg")
                    .when(pending, ui::MenuItem::disabled)
                    .on_click(move |_, _, cx| {
                        library.update(cx, |library, cx| {
                            library.set_sidebar_pinned(uri.clone(), !pinned, cx)
                        });
                    }),
                );
            }
            (menu, *position)
        } else {
            let (pin, position) = self.context_menu.clone()?;
            (
                pin_menu(&pin, &self.track_menu, self.playback.clone(), cx),
                position,
            )
        };

        Some(
            Popup::new(position, menu)
                .on_close(cx.listener(|this, _, _, cx| this.dismiss_menu(cx))),
        )
    }

    pub fn is_open(&self) -> bool {
        self.forced.unwrap_or(self.open && !self.cramped)
    }

    pub fn overlays(&self) -> bool {
        self.cramped && self.is_open()
    }

    pub fn overlay_width(&self) -> Pixels {
        match self.overlays() {
            true => self.width,
            false => Pixels::ZERO,
        }
    }

    fn dismiss(&mut self, cx: &mut Context<Self>) {
        if !self.overlays() {
            return;
        }
        self.forced = Some(false);
        cx.notify();
    }

    pub fn occupied_width(&self) -> Pixels {
        match self.is_open() && !self.overlays() {
            true => self.width,
            false => Pixels::ZERO,
        }
    }

    pub fn toggle(&mut self, cx: &mut Context<Self>) {
        match self.cramped {
            true => self.forced = Some(!self.is_open()),
            false => {
                self.open = !self.open;
                self.persist(cx);
            }
        }
        cx.notify();
    }

    fn ceiling(&self, window: &Window, cx: &Context<Self>) -> Pixels {
        let reserved = match self.overlays() {
            true => Pixels::ZERO,
            false => SNUG + super::Chrome::sidebar_right(cx),
        };

        super::cap(MIN_WIDTH, MAX_WIDTH, reserved, window)
    }

    pub fn adapt(&mut self, window: &Window, cx: &mut Context<Self>) {
        self.width = ui::snapped(self.width, window);

        let taken = self.width + super::Chrome::sidebar_right(cx);
        let space_left = window.viewport_size().width - taken;
        let cramped = space_left < SNUG;
        if cramped != self.cramped {
            self.cramped = cramped;
            self.forced = None;
        }
    }

    fn pin_row(&self, index: usize, pin: Pin, count: usize, cx: &mut Context<Self>) -> AnyElement {
        let destination = Destination::from(&pin);
        let active = destination == self.trail.read(cx).current();
        let opened = pin.clone();
        let edge = match self.drop_gap {
            Some(gap) if gap == index => Some(Edge::Above),
            Some(gap) if gap == count && index + 1 == count => Some(Edge::Below),
            _ => None,
        };

        let origin = Origin::from(&pin);
        let playing = matches!(
            self.playback.read(cx).playing_from(&origin),
            Some(PlaybackState::Playing)
        );

        let card = sidebar_card(("pinned", index), pin.label(), active, cx)
            .cover(pin.cover.clone())
            .fallback(pin.kind.icon())
            .when(pin.kind.round(), Card::circle)
            .play(
                playing,
                cx.listener(move |this, _, _, cx| {
                    this.playback
                        .update(cx, |playback, cx| playback.toggle_origin(&origin, cx));
                }),
            )
            .meta(library_caption(pin.kind.key(), "", true, cx))
            .press(move |_, _, cx| navigate(destination.clone(), cx))
            .menu(cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                this.track_menu.reset(cx);
                this.library_context_menu = this
                    .library
                    .read(cx)
                    .sidebar_items()
                    .and_then(|items| {
                        items
                            .iter()
                            .find(|item| item_pin(item).is_some_and(|pin| pin.same(&opened)))
                    })
                    .cloned()
                    .map(|item| (item, event.position));
                this.context_menu = Some((opened.clone(), event.position));
                cx.notify();
            }))
            .pin(pin)
            .on_drag_move(
                cx.listener(move |this, event: &DragMoveEvent<DraggedPin>, _, cx| {
                    let Some(gap) = drop_gap(event.bounds, event.event.position, index) else {
                        return;
                    };
                    let dragged = event.drag(cx).pin.clone();
                    let slugs = this.session.read(cx).active_slugs();
                    let from = this
                        .settings
                        .read(cx)
                        .pinned(&slugs)
                        .iter()
                        .position(|it| it.same(&dragged));
                    let gap = match from {
                        Some(from) if gap == from || gap == from + 1 => None,
                        _ => Some(gap),
                    };
                    if this.drop_gap != gap {
                        this.drop_gap = gap;
                        cx.notify();
                    }
                }),
            );

        div()
            .id(("pinned-slot", index))
            .relative()
            .flex_none()
            .w_full()
            .min_w_0()
            .child(card)
            .when_some(edge, |this, edge| this.child(drop_marker(edge, cx)))
            .into_any_element()
    }

    fn library_order(&self, local_pins: &[Pin], cx: &App) -> Vec<LibraryItem> {
        let library = self.library.read(cx);
        let items = library
            .sidebar_items()
            .map(<[LibraryItem]>::to_vec)
            .unwrap_or_else(|| fallback_library(library.state(Shelf::Streaming)));
        without_local_pins(items, local_pins)
    }

    fn library_toolbar(&self, empty: bool, cx: &mut Context<Self>) -> AnyElement {
        let library = self.library.read(cx);
        let loading = matches!(library.state(Shelf::Streaming), LibraryState::Loading);
        let empty_key = if loading {
            "play-loading"
        } else if library.part_failed(Shelf::Streaming, state::LibraryPart::Playlists)
            || matches!(library.state(Shelf::Streaming), LibraryState::Failed(_))
        {
            "library-part-not-loaded"
        } else {
            "library-no-matches"
        };
        let theme = *cx.theme();
        div()
            .flex()
            .flex_col()
            .flex_none()
            .w_full()
            .min_w_0()
            .min_h_0()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .w_full()
                    .min_w_0()
                    .flex_none()
                    .h(theme.metrics.control_small)
                    .px_3()
                    .gap(theme.metrics.pad / 2.)
                    .child(
                        div().flex_1().min_w_0().overflow_hidden().child(
                            ui::eyebrow(i18n::lookup("nav-library", None), cx)
                                .w_full()
                                .truncate(),
                        ),
                    )
                    .when(self.session.read(cx).authenticated(), |row| {
                        let selected = self.library.read(cx).sidebar_order();
                        let library = self.library.clone();
                        row.child(
                            ui::Popover::new(
                                "sidebar-library-order",
                                self.library_popovers.clone(),
                            )
                            .max_w(gpui::relative(0.55))
                            .min_w_0()
                            .flex_none()
                            .commands()
                            .button(
                                Button::new("sidebar-library-sort")
                                    .ghost()
                                    .small()
                                    .max_w(gpui::relative(1.))
                                    .min_w_0()
                                    .label(i18n::lookup(order_key(selected), None))
                                    .trailing("icons/list.svg")
                                    .text_right()
                                    .text_size(theme.text(Text::Tiny))
                                    .tint(theme.muted_foreground),
                            )
                            .menu(
                                ui::Menu::new("sidebar-library-order-menu")
                                    .right_0()
                                    .top(theme.metrics.control_small)
                                    .items(LibraryOrder::ALL.into_iter().map(move |order| {
                                        let library = library.clone();
                                        ui::MenuItem::new(
                                            order_key(order),
                                            i18n::lookup(order_key(order), None),
                                        )
                                        .selected(order == selected)
                                        .on_click(
                                            move |_, _, cx| {
                                                library.update(cx, |library, cx| {
                                                    library.set_sidebar_order(order, cx)
                                                });
                                            },
                                        )
                                    })),
                            ),
                        )
                    }),
            )
            .when(empty && self.dropping, |view| view.child(hint(cx)))
            .when(empty && !self.dropping, |view| {
                view.child(
                    div()
                        .px_3()
                        .py_2()
                        .text_size(theme.text(Text::Small))
                        .text_color(theme.muted_foreground)
                        .child(i18n::lookup(empty_key, None)),
                )
            })
            .into_any_element()
    }

    fn library_card(&self, item: LibraryItem, cx: &mut Context<Self>) -> AnyElement {
        let context_item = item.clone();
        let origin = item_origin(&item);
        let destination = item_destination(&item);
        let active = destination.as_ref() == Some(&self.trail.read(cx).current());
        let playing = matches!(
            origin
                .as_ref()
                .and_then(|origin| self.playback.read(cx).playing_from(origin)),
            Some(PlaybackState::Playing)
        );
        sidebar_card(
            gpui::SharedString::from(format!("sidebar-library-{}", item.uri)),
            item.name.clone().into(),
            active,
            cx,
        )
        .cover(item.cover.clone())
        .fallback(item_icon(item.kind))
        .when(item.kind == LibraryItemKind::Artist, Card::circle)
        .meta(library_caption(
            item_key(item.kind),
            &item.subtitle,
            item.pinned,
            cx,
        ))
        .when_some(origin, |card, origin| {
            card.play(
                playing,
                cx.listener(move |this, _, _, cx| {
                    this.playback
                        .update(cx, |playback, cx| playback.toggle_origin(&origin, cx));
                }),
            )
        })
        .press(move |_, _, cx| match &destination {
            Some(destination) => navigate(destination.clone(), cx),
            None => cx.open_url(&spotify_url(&item.uri)),
        })
        .menu(cx.listener(move |this, event: &MouseDownEvent, _, cx| {
            this.track_menu.reset(cx);
            this.context_menu = None;
            this.library_context_menu = Some((context_item.clone(), event.position));
            cx.notify();
        }))
        .into_any_element()
    }

    fn navigation(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = *cx.theme();
        let sidebar_accent = theme.sidebar_accent;
        let foreground = theme.foreground;
        let muted = theme.muted_foreground;
        let current = self.trail.read(cx).current();
        let authenticated = self.session.read(cx).authenticated();
        let shown = |entry: NavEntry, cx: &App| self.settings.read(cx).nav_shown(entry.id());

        let mut rows: Vec<AnyElement> = Vec::new();
        for (index, (entry, icon, destination)) in NAV.into_iter().enumerate() {
            let key = entry.map_or("nav-settings", NavEntry::key);
            if entry.is_some_and(|entry| !shown(entry, cx)) {
                continue;
            }

            if matches!(destination, Destination::Library(_)) {
                if !authenticated {
                    continue;
                }
                let inside = matches!(current, Destination::Library(_));
                let text = if inside { foreground } else { muted };

                rows.push(
                    nav_row(index, key, text, sidebar_accent)
                        .icon(icon)
                        .trailing(chevron(self.library_open))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.library_open = !this.library_open;
                            cx.notify();
                        }))
                        .into_any_element(),
                );

                if self.library_open {
                    rows.push(
                        Tabs::new()
                            .items(LIBRARY_TABS.into_iter().map(|(name, tab)| {
                                let chosen = current == Destination::Library(tab);
                                let tint = if chosen { foreground } else { muted };

                                nav_row(name, name, tint, sidebar_accent)
                                    .flex_1()
                                    .when(chosen, |button| button.bg(sidebar_accent))
                                    .on_click(move |_, _, cx| {
                                        navigate(Destination::Library(tab), cx)
                                    })
                            }))
                            .into_any_element(),
                    );
                }
                continue;
            }

            if matches!(destination, Destination::Local(_)) {
                let inside = matches!(current, Destination::Local(_));
                let text = if inside { foreground } else { muted };

                rows.push(
                    nav_row(index, key, text, sidebar_accent)
                        .icon(icon)
                        .trailing(chevron(self.local_open))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.local_open = !this.local_open;
                            cx.notify();
                        }))
                        .into_any_element(),
                );

                if self.local_open {
                    rows.push(
                        Tabs::new()
                            .items(LIBRARY_TABS.into_iter().enumerate().map(
                                |(slot, (name, tab))| {
                                    let chosen = current == Destination::Local(tab);
                                    let tint = if chosen { foreground } else { muted };

                                    nav_row(("local-tab", slot as u32), name, tint, sidebar_accent)
                                        .flex_1()
                                        .when(chosen, |button| button.bg(sidebar_accent))
                                        .on_click(move |_, _, cx| {
                                            navigate(Destination::Local(tab), cx)
                                        })
                                },
                            ))
                            .into_any_element(),
                    );
                }
                continue;
            }

            if matches!(destination, Destination::Settings(_)) {
                let inside = matches!(current, Destination::Settings(_));
                let text = if inside { foreground } else { muted };

                rows.push(
                    nav_row(index, key, text, sidebar_accent)
                        .icon(icon)
                        .trailing(chevron(self.settings_open))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.settings_open = !this.settings_open;
                            cx.notify();
                        }))
                        .into_any_element(),
                );

                if self.settings_open {
                    rows.push(
                        Tabs::new()
                            .items(SETTINGS_TABS.into_iter().map(|(name, tab)| {
                                let chosen = current == Destination::Settings(tab);
                                let tint = if chosen { foreground } else { muted };

                                nav_row(name, name, tint, sidebar_accent)
                                    .flex_1()
                                    .when(chosen, |button| button.bg(sidebar_accent))
                                    .on_click(move |_, _, cx| {
                                        navigate(Destination::Settings(tab), cx)
                                    })
                            }))
                            .into_any_element(),
                    );
                }
                continue;
            }

            let active = destination.same_section(&current);
            let text = if active { foreground } else { muted };

            rows.push(
                nav_row(index, key, text, sidebar_accent)
                    .icon(icon)
                    .when(active, |button| button.bg(sidebar_accent))
                    .on_click(move |_, _, cx| navigate(destination.clone(), cx))
                    .into_any_element(),
            );
        }

        div()
            .flex()
            .flex_col()
            .flex_none()
            .gap_1()
            .w_full()
            .p_3()
            .children(rows)
            .into_any_element()
    }

    fn return_to_top(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let viewport = self.scroll.viewport_bounds().size.height;
        let threshold = (cx.theme().metrics.list_row * 3.).min(viewport / 2.);
        if viewport <= Pixels::ZERO || -self.scroll.scroll_px_offset_for_scrollbar().y < threshold {
            return None;
        }
        let theme = *cx.theme();
        Some(
            div()
                .absolute()
                .bottom_3()
                .w_full()
                .flex()
                .justify_center()
                .child(
                    div().flex().flex_none().block_mouse_except_scroll().child(
                        Button::new("sidebar-return-top")
                            .ghost()
                            .small()
                            .icon("icons/undo-2.svg")
                            .tooltip("nav-return-top")
                            .rounded_full()
                            .border_1()
                            .border_color(theme.border)
                            .bg(theme.popover)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.scrollbar
                                    .update(cx, |bar, _| bar.aim(Pixels::ZERO, window));
                                cx.notify();
                            })),
                    ),
                ),
        )
    }

    fn persist(&self, cx: &mut Context<Self>) {
        let width = self.width / px(1.);
        let open = self.open;
        self.settings
            .update(cx, |settings, cx| settings.set_sidebar(width, open, cx));
    }
}

impl Render for SidebarLeft {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.theme();
        let sidebar_bg = theme.sidebar;
        let sidebar_border = theme.sidebar_border;

        let current = self.trail.read(cx).current();
        self.follow(&current);
        let authenticated = self.session.read(cx).authenticated();
        self.adapt(window, cx);

        if !cx.has_active_drag() {
            self.dropping = false;
            self.drop_gap = None;
        }

        let slugs = self.session.read(cx).active_slugs();
        let library_enabled = self.settings.read(cx).nav_shown(NavEntry::LibraryList.id());
        let local_pins = if library_enabled {
            self.settings.read(cx).pinned(&slugs)
        } else {
            Vec::new()
        };
        let local_count = local_pins.len();
        let order = if authenticated && library_enabled {
            self.library_order(&local_pins, cx)
        } else {
            Vec::new()
        };
        let show_library = library_enabled && (authenticated || local_count > 0 || self.dropping);
        let count = 1 + usize::from(show_library) + local_count + order.len();
        let card_height = theme.metrics.list_row + window.rem_size() * 0.25;
        let old_count = self.scroll.item_count();
        if count != old_count {
            self.scroll.splice(
                count.min(old_count)..old_count,
                count.saturating_sub(old_count),
            );
            // Off-screen cards still contribute to the scroll range.
            self.scroll.clone().with_uniform_item_height(card_height);
        }
        // Navigation can expand and the empty-state message can change height.
        // Remeasure only these two rows; library cards retain their cached heights.
        self.scroll.remeasure_items(0..count.min(2));
        if self.row_height != theme.metrics.list_row {
            self.row_height = theme.metrics.list_row;
            self.scroll.remeasure();
        }
        // GPUI clears height hints when it measures a new list width. Restore
        // the card estimate after layout, without building those cards.
        let sidebar = cx.entity().downgrade();
        window.on_next_frame(move |_, cx| {
            sidebar
                .update(cx, |this, cx| {
                    let width = this.scroll.viewport_bounds().size.width;
                    if this.scroll_width != Some(width) {
                        this.scroll_width = Some(width);
                        this.scroll.clone().with_uniform_item_height(card_height);
                        cx.notify();
                    }
                })
                .ok();
        });
        self.scrollbar.read(cx).sync();
        let gliding = self.scrollbar.clone();
        let content = list(
            self.scroll.clone(),
            cx.processor(move |this, index, _, cx| match index {
                0 => this.navigation(cx),
                1 => this.library_toolbar(local_count == 0 && order.is_empty(), cx),
                _ => {
                    let row = index - 2;
                    let card = if row < local_count {
                        this.pin_row(row, local_pins[row].clone(), local_count, cx)
                    } else {
                        this.library_card(order[row - local_count].clone(), cx)
                    };
                    div().w_full().px_3().pb_1().child(card).into_any_element()
                }
            }),
        )
        .size_full();

        let overlaid = self.overlays();
        let panel = Panel::new("sidebar-left", Side::Left, self.width)
            .limits(MIN_WIDTH, MAX_WIDTH)
            .reach(self.ceiling(window, cx))
            .clears_scrollbar()
            .on_resize(cx.listener(|this, width: &Pixels, _, cx| {
                this.width = *width;
                this.persist(cx);
                cx.notify();
            }))
            .on_drag_move(cx.listener(|this, _: &DragMoveEvent<DraggedPin>, _, cx| {
                let settled = this.drop_gap.take().is_some() || !this.dropping;
                this.dropping = true;
                if settled {
                    cx.notify();
                }
            }))
            .on_drop(cx.listener(|this, dragged: &DraggedPin, _, cx| {
                let gap = this.drop_gap.take();
                this.dropping = false;
                let pin = dragged.pin.clone();
                if let Some(slug) = this.session.read(cx).slug_for(&pin.id) {
                    let slugs = this.session.read(cx).active_slugs();
                    this.settings
                        .update(cx, |settings, cx| settings.pin(slug, pin, gap, &slugs, cx));
                }
                cx.notify();
            }))
            .when(!self.is_open(), |this| this.hidden())
            .when(!theme.transparent, |this| this.bg(sidebar_bg))
            .border_color(sidebar_border)
            .when(overlaid, |this| {
                this.occlude().absolute().left_0().top_0().bottom_0()
            })
            .child(
                div()
                    .relative()
                    .size_full()
                    .min_h_0()
                    .child(content)
                    .on_scroll_wheel(move |event: &ScrollWheelEvent, window, cx| {
                        gliding.update(cx, |bar, _| {
                            if event.delta.precise() {
                                bar.stirred();
                            } else {
                                bar.nudge(window);
                            }
                        });
                    })
                    .child(self.scrollbar.clone())
                    .children(self.return_to_top(cx)),
            )
            .children(self.menu(cx));

        match overlaid {
            false => panel.into_any_element(),
            true => div()
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .bottom_0()
                .child(
                    Shield::new("sidebar-shield")
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .bottom_0()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _: &MouseDownEvent, _, cx| this.dismiss(cx)),
                        ),
                )
                .child(panel)
                .into_any_element(),
        }
    }
}

fn library_caption(kind: &str, subtitle: &str, pinned: bool, cx: &App) -> impl IntoElement {
    let theme = *cx.theme();
    let kind = i18n::lookup(kind, None);
    let label = if subtitle.is_empty() {
        kind.to_string()
    } else {
        format!("{kind} · {subtitle}")
    };
    div()
        .flex()
        .items_center()
        .gap(theme.metrics.pad / 4.)
        .min_w_0()
        .when(pinned, |row| {
            row.child(
                svg()
                    .path(icons::path("icons/pin.svg"))
                    .flex_none()
                    .size(theme.text(Text::Tiny))
                    .text_color(theme.pinned),
            )
        })
        .child(div().min_w_0().truncate().child(label))
}

fn order_key(order: LibraryOrder) -> &'static str {
    match order {
        LibraryOrder::Recents => "nav-library-recents",
        LibraryOrder::RecentlyAdded => "nav-library-added",
        LibraryOrder::Alphabetical => "nav-library-alphabetical",
        LibraryOrder::Creator => "nav-library-creator",
    }
}

fn sidebar_card(
    id: impl Into<ElementId>,
    title: gpui::SharedString,
    active: bool,
    cx: &App,
) -> Card {
    let theme = *cx.theme();
    let accent = theme.sidebar_accent;
    Card::new(id, title)
        .tint(if active {
            theme.foreground
        } else {
            theme.muted_foreground
        })
        .when(active, |card| card.bg(accent))
        .hover(move |style| style.bg(accent))
}

fn without_local_pins(mut items: Vec<LibraryItem>, local_pins: &[Pin]) -> Vec<LibraryItem> {
    items.retain(|item| {
        item.kind != LibraryItemKind::LikedSongs
            && item_pin(item).is_none_or(|pin| !local_pins.iter().any(|local| local.same(&pin)))
    });
    items
}

fn item_pin(item: &LibraryItem) -> Option<Pin> {
    let kind = match item.kind {
        LibraryItemKind::Playlist => ui::PinKind::Playlist,
        LibraryItemKind::Album => ui::PinKind::Album,
        LibraryItemKind::Artist => ui::PinKind::Artist,
        _ => return None,
    };
    let tail = item.uri.strip_prefix("spotify:").unwrap_or(&item.uri);
    let (_, id) = tail.split_once(':')?;
    Some(Pin::new(kind, id, item.name.clone()).cover(item.cover.clone()))
}

fn item_destination(item: &LibraryItem) -> Option<Destination> {
    if item.kind == LibraryItemKind::LikedSongs {
        return Some(Destination::Library(LibraryTab::Songs));
    }
    item_pin(item).as_ref().map(Destination::from)
}

fn item_origin(item: &LibraryItem) -> Option<Origin> {
    if item.kind == LibraryItemKind::LikedSongs {
        return Some(Origin::saved());
    }
    item_pin(item).as_ref().map(Origin::from)
}

fn item_key(kind: LibraryItemKind) -> &'static str {
    match kind {
        LibraryItemKind::Playlist | LibraryItemKind::LikedSongs => "kind-playlist",
        LibraryItemKind::Album => "kind-album",
        LibraryItemKind::Artist => "kind-artist",
        LibraryItemKind::Audiobook => "kind-audiobook",
        LibraryItemKind::Show => "kind-podcast",
        LibraryItemKind::Folder => "kind-folder",
    }
}

fn item_icon(kind: LibraryItemKind) -> &'static str {
    match kind {
        LibraryItemKind::Artist => "icons/user.svg",
        LibraryItemKind::Album => "icons/disc-3.svg",
        LibraryItemKind::LikedSongs => "icons/heart-filled.svg",
        _ => "icons/list.svg",
    }
}

fn spotify_url(uri: &str) -> String {
    match uri
        .strip_prefix("spotify:")
        .and_then(|tail| tail.split_once(':'))
    {
        Some((kind @ ("show" | "album" | "artist" | "playlist"), id)) => {
            format!("https://open.spotify.com/{kind}/{id}")
        }
        _ => uri.to_owned(),
    }
}

fn fallback_library(shelf: &LibraryState) -> Vec<LibraryItem> {
    let playlists = shelf
        .playlists()
        .iter()
        .filter_map(|item| Some((item.pin()?, item.owner.clone())));
    let albums = shelf
        .albums()
        .iter()
        .filter_map(|item| Some((item.pin()?, item.artists.clone())));
    let artists = shelf
        .artists()
        .iter()
        .filter_map(|item| Some((item.pin()?, String::new())));
    playlists
        .chain(albums)
        .chain(artists)
        .map(|(pin, subtitle)| {
            let (kind, prefix) = match pin.kind {
                ui::PinKind::Album => (LibraryItemKind::Album, "album"),
                ui::PinKind::Artist => (LibraryItemKind::Artist, "artist"),
                _ => (LibraryItemKind::Playlist, "playlist"),
            };
            LibraryItem {
                uri: format!("{prefix}:{}", pin.id),
                name: pin.title,
                subtitle,
                cover: pin.cover,
                kind,
                pinned: false,
            }
        })
        .collect()
}

fn hint(cx: &App) -> AnyElement {
    let theme = *cx.theme();

    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .h(HINT_HEIGHT)
        .mx_2()
        .px_2()
        .rounded(theme.radius)
        .border_1()
        .border_dashed()
        .border_color(theme.sidebar_border)
        .text_size(theme.text(Text::Small))
        .text_color(theme.muted_foreground)
        .text_center()
        .child(i18n::lookup("nav-pin-hint", None))
        .into_any_element()
}

fn expanded(current: &Destination) -> (bool, bool, bool) {
    (
        matches!(current, Destination::Library(_)),
        matches!(current, Destination::Local(_)),
        matches!(current, Destination::Settings(_)),
    )
}

fn chevron(open: bool) -> &'static str {
    match open {
        true => "icons/chevron-down.svg",
        false => "icons/chevron-right.svg",
    }
}

fn nav_row(id: impl Into<ElementId>, key: &'static str, tint: Hsla, accent: Hsla) -> Button {
    Button::new(id)
        .ghost()
        .label(i18n::lookup(key, None))
        .tint(tint)
        .gap_2p5()
        .justify_start()
        .hover(move |style| style.bg(accent))
        .active(move |style| style.bg(accent))
}

#[cfg(test)]
mod tests {
    use router::{Destination, LibraryTab, SettingsTab};

    use super::expanded;

    #[test]
    fn merged_library_keeps_local_pins_once_and_preserves_spotify_order() {
        use music::{LibraryItem, LibraryItemKind};
        use ui::{Pin, PinKind};
        let item = |id: &str, kind, pinned| LibraryItem {
            uri: format!(
                "spotify:{}:{id}",
                if kind == LibraryItemKind::Artist {
                    "artist"
                } else {
                    "playlist"
                }
            ),
            name: id.to_owned(),
            subtitle: String::new(),
            cover: None,
            kind,
            pinned,
        };
        let local = vec![
            Pin::new(PinKind::Artist, "shared", "Local artist"),
            Pin::new(PinKind::Song, "song", "Local song"),
        ];
        let rows = super::without_local_pins(
            vec![
                item("shared", LibraryItemKind::Artist, true),
                item("first", LibraryItemKind::Playlist, true),
                item("liked", LibraryItemKind::LikedSongs, true),
                item("shared", LibraryItemKind::Playlist, false),
                item("last", LibraryItemKind::Artist, false),
            ],
            &local,
        );
        assert_eq!(
            rows.iter()
                .map(|item| item.name.as_str())
                .collect::<Vec<_>>(),
            vec!["first", "shared", "last"]
        );
        assert!(rows[0].pinned);
    }

    #[test]
    fn a_section_expands_only_where_it_leads() {
        assert_eq!(
            expanded(&Destination::Library(LibraryTab::Albums)),
            (true, false, false)
        );
        assert_eq!(
            expanded(&Destination::Local(LibraryTab::Albums)),
            (false, true, false)
        );
        assert_eq!(
            expanded(&Destination::Settings(SettingsTab::General)),
            (false, false, true)
        );
    }

    #[test]
    fn content_belongs_to_neither_section() {
        let away = [
            Destination::Home,
            Destination::Search,
            Destination::Album("id".into()),
            Destination::Playlist("id".into()),
            Destination::Artist("id".into()),
            Destination::Song("id".into()),
        ];

        for destination in away {
            assert_eq!(
                expanded(&destination),
                (false, false, false),
                "{destination:?}"
            );
        }
    }
}

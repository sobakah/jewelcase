//! The album or artist grid, and apart from the library, collapsible
//! sections before or after it: Spotify's saved albums or followed artists
//! at the end, new releases at the top (CONCEPT.md §9.8).
//!
//! GTK's grid view has no sections, so this is a list view of rows, each
//! a line of tiles: only the rows on screen are built, as in a grid view,
//! and a row can be a full-width header. Tiles keep the cover size set in
//! Preferences: a width change only changes the number of columns (the
//! rows are laid out again then) and the room between them.

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gettextrs::gettext;
use gtk::subclass::prelude::*;
use gtk::{gio, glib};

use super::{ShelfItem, tile_content};
use crate::application::JewelcaseApplication;
use crate::config::APP_ID;

/// The grid's padding on each side (style.css: .album-grid, .artist-grid).
const PADDING: i32 = 12;
const MIN_COLUMNS: i32 = 2;
const MAX_COLUMNS: i32 = 40;

/// One line of the list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    /// Library items `start..start + len`.
    Local { start: u32, len: u32 },
    /// A section's header ("Saved on Spotify (N)"), which opens and
    /// closes it.
    Header(usize),
    /// A section's items `start..start + len`.
    Section {
        section: usize,
        start: u32,
        len: u32,
    },
}

/// Where a section goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    Before,
    After,
}

/// A part of the grid apart from the library.
struct Section {
    place: Place,
    /// "Saved on Spotify ({})".
    title: String,
    /// The boolean setting that keeps it open.
    key: String,
    items: Rc<Vec<ShelfItem>>,
    activate: Option<SpotifyActivate>,
    /// Shows a way to connect to Spotify again.
    needs_permission: bool,
    /// The items changed: their rows show other items now.
    dirty: bool,
    /// The row showing the header, to update it in place.
    header_row: glib::WeakRef<GridRow>,
}

type Create = Rc<dyn Fn() -> gtk::Widget>;
type Bind = Rc<dyn Fn(&gtk::Widget, &glib::Object)>;
type Activate = Rc<dyn Fn(&glib::Object)>;
type SpotifyActivate = Rc<dyn Fn(usize)>;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct SectionedGrid {
        pub scroller: gtk::ScrolledWindow,
        pub list: gtk::ListView,
        pub rows: OnceCell<gio::ListStore>,
        pub local: RefCell<Option<gio::ListModel>>,
        pub(super) sections: RefCell<Vec<Section>>,
        pub create: OnceCell<Create>,
        pub bind: OnceCell<Bind>,
        pub activate: OnceCell<Activate>,
        /// A tile's padding on both sides (style.css: .grid-tile).
        pub tile_padding: Cell<i32>,
        /// 0 until the first width is known (rows built for a guess would
        /// show huge tiles for a frame). Shared with the rows, which size
        /// their tiles by it.
        pub columns: Rc<Cell<u32>>,
        pub rebuild_pending: Cell<bool>,
        pub settings: OnceCell<gio::Settings>,
        /// The rows in the list now, so a rebuild only replaces what
        /// changed (rows that stay keep their widgets, and focus).
        pub shown: RefCell<Vec<Row>>,
        /// The library's items changed: their rows show other items now.
        pub local_dirty: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SectionedGrid {
        const NAME: &'static str = "JewelcaseSectionedGrid";
        type Type = super::SectionedGrid;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for SectionedGrid {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            self.tile_padding.set(18);
            let rows = gio::ListStore::new::<glib::BoxedAnyObject>();
            let factory = gtk::SignalListItemFactory::new();
            let columns = self.columns.clone();
            factory.connect_setup(move |_, item| {
                let item = item.downcast_ref::<gtk::ListItem>().expect("list item");
                // Tiles take the focus and clicks, not the rows.
                item.set_activatable(false);
                item.set_selectable(false);
                item.set_focusable(false);
                let row = GridRow::default();
                row.imp().columns.replace(Some(columns.clone()));
                item.set_child(Some(&row));
            });
            factory.connect_bind(glib::clone!(
                #[weak]
                obj,
                move |_, item| {
                    let item = item.downcast_ref::<gtk::ListItem>().expect("list item");
                    let (Some(widget), Some(row)) = (
                        item.child().and_downcast::<GridRow>(),
                        item.item().and_downcast::<glib::BoxedAnyObject>(),
                    ) else {
                        return;
                    };
                    let row = *row.borrow::<Row>();
                    obj.bind_row(&widget, row);
                }
            ));
            self.list.set_factory(Some(&factory));
            self.list
                .set_model(Some(&gtk::NoSelection::new(Some(rows.clone()))));
            self.list.add_css_class("sectioned-grid");
            let _ = self.rows.set(rows);
            self.scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
            self.scroller.set_child(Some(&self.list));
            self.scroller.set_parent(&*obj);
            super::super::connect_tile_size(&*obj, |grid| grid.queue_resize());
        }

        fn dispose(&self) {
            self.scroller.unparent();
        }
    }

    impl WidgetImpl for SectionedGrid {
        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            self.scroller.measure(orientation, for_size)
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            self.scroller.allocate(width, height, baseline, None);
            let available = width - 2 * PADDING;
            let column = super::super::tile_size() + self.tile_padding.get();
            let columns = (available / column.max(1)).clamp(MIN_COLUMNS, MAX_COLUMNS) as u32;
            if columns != self.columns.get() {
                self.columns.set(columns);
                // Not while allocating: the rows change the list.
                self.obj().rebuild_soon();
            }
        }
    }
}

glib::wrapper! {
    pub struct SectionedGrid(ObjectSubclass<imp::SectionedGrid>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl SectionedGrid {
    /// The library part: `create` makes a tile, `bind` shows an item in it,
    /// `activate` opens an item. `tile_padding` is a tile's padding on both
    /// sides;
    /// `class` the grid's style class ("album-grid", "artist-grid").
    pub fn setup(
        &self,
        model: &impl IsA<gio::ListModel>,
        tile_padding: i32,
        class: &str,
        create: impl Fn() -> gtk::Widget + 'static,
        bind: impl Fn(&gtk::Widget, &glib::Object) + 'static,
        activate: impl Fn(&glib::Object) + 'static,
    ) {
        let imp = self.imp();
        imp.tile_padding.set(tile_padding);
        imp.list.add_css_class(class);
        let _ = imp.create.set(Rc::new(create));
        let _ = imp.bind.set(Rc::new(bind));
        let _ = imp.activate.set(Rc::new(activate));
        let model = model.clone().upcast::<gio::ListModel>();
        model.connect_items_changed(glib::clone!(
            #[weak(rename_to = grid)]
            self,
            move |_, _, _, _| {
                grid.imp().local_dirty.set(true);
                grid.rebuild_soon();
            }
        ));
        imp.local.replace(Some(model));
        self.rebuild_soon();
    }

    /// Scrolls to the end (screenshot tests of the Spotify sections).
    pub fn scroll_to_end(&self) {
        let adjustment = self.imp().scroller.vadjustment();
        adjustment.set_value(adjustment.upper());
    }

    /// Clicks the first tile on screen (screenshot tests).
    pub fn click_first_tile(&self) {
        fn find(widget: &gtk::Widget) -> Option<gtk::Button> {
            if widget.has_css_class("grid-tile") {
                return widget.clone().downcast().ok();
            }
            let mut child = widget.first_child();
            while let Some(current) = child {
                if let Some(found) = find(&current) {
                    return Some(found);
                }
                child = current.next_sibling();
            }
            None
        }
        if let Some(button) = find(self.imp().list.upcast_ref()) {
            button.emit_clicked();
        }
    }

    /// The library's items.
    pub fn local_model(&self) -> Option<gio::ListModel> {
        self.imp().local.borrow().clone()
    }

    /// Adds a section `place`d before or after the library: `title` with
    /// a `{}` for the count; `key` the boolean setting that keeps it open.
    /// Returns its index for `set_section`.
    pub fn add_section(&self, place: Place, title: &str, key: &str) -> usize {
        let imp = self.imp();
        let settings = imp.settings.get_or_init(|| gio::Settings::new(APP_ID));
        settings.connect_changed(
            Some(key),
            glib::clone!(
                #[weak(rename_to = grid)]
                self,
                move |_, _| grid.rebuild_soon()
            ),
        );
        let mut sections = imp.sections.borrow_mut();
        sections.push(Section {
            place,
            title: title.to_owned(),
            key: key.to_owned(),
            items: Rc::default(),
            activate: None,
            needs_permission: false,
            dirty: false,
            header_row: glib::WeakRef::new(),
        });
        sections.len() - 1
    }

    /// A section's items (shown only when there are any, or with
    /// `needs_permission`, which asks to connect to Spotify again);
    /// `activate` opens one.
    pub fn set_section(
        &self,
        section: usize,
        items: Vec<ShelfItem>,
        needs_permission: bool,
        activate: impl Fn(usize) + 'static,
    ) {
        if let Some(s) = self.imp().sections.borrow_mut().get_mut(section) {
            s.items = Rc::new(items);
            s.dirty = true;
            s.needs_permission = needs_permission;
            s.activate = Some(Rc::new(activate));
        }
        self.rebuild_soon();
    }

    fn section_open(&self, section: usize) -> bool {
        let imp = self.imp();
        let key = imp.sections.borrow().get(section).map(|s| s.key.clone());
        match (imp.settings.get(), key) {
            (Some(settings), Some(key)) => settings.boolean(&key),
            _ => false,
        }
    }

    fn rebuild_soon(&self) {
        let imp = self.imp();
        if imp.rebuild_pending.replace(true) {
            return;
        }
        glib::idle_add_local_once(glib::clone!(
            #[weak(rename_to = grid)]
            self,
            move || {
                grid.imp().rebuild_pending.set(false);
                grid.rebuild();
            }
        ));
    }

    /// The rows for the current items and column count.
    fn rebuild(&self) {
        let imp = self.imp();
        let columns = imp.columns.get();
        if columns == 0 {
            // Not laid out yet: no rows rather than rows for a guess.
            return;
        }
        let chunks = |count: u32, make: &dyn Fn(u32, u32) -> Row| -> Vec<Row> {
            (0..count)
                .step_by(columns as usize)
                .map(|start| make(start, columns.min(count - start)))
                .collect()
        };
        let local = imp
            .local
            .borrow()
            .as_ref()
            .map_or(0, gio::ListModel::n_items);
        let (places, counts): (Vec<Place>, Vec<(u32, bool)>) = imp
            .sections
            .borrow()
            .iter()
            .map(|s| (s.place, (s.items.len() as u32, s.needs_permission)))
            .unzip();
        let section_rows = |place: Place| -> Vec<Row> {
            let mut rows = Vec::new();
            for (section, &(count, needs_permission)) in counts.iter().enumerate() {
                if places[section] != place || (count == 0 && !needs_permission) {
                    continue;
                }
                rows.push(Row::Header(section));
                if self.section_open(section) {
                    rows.extend(chunks(count, &|start, len| Row::Section {
                        section,
                        start,
                        len,
                    }));
                }
            }
            rows
        };
        let mut rows = section_rows(Place::Before);
        rows.extend(chunks(local, &|start, len| Row::Local { start, len }));
        rows.extend(section_rows(Place::After));
        // Only what changed: rows that stay keep their widgets (no new
        // textures, focus kept); changed items change every row after them.
        let old = imp.shown.replace(rows.clone());
        let local_dirty = imp.local_dirty.replace(false);
        let dirty: Vec<bool> = imp
            .sections
            .borrow_mut()
            .iter_mut()
            .map(|s| std::mem::take(&mut s.dirty))
            .collect();
        let keep = old
            .iter()
            .zip(&rows)
            .take_while(|(a, b)| {
                a == b
                    && match a {
                        Row::Local { .. } => !local_dirty,
                        Row::Section { section, .. } => !dirty[*section],
                        Row::Header(_) => true,
                    }
            })
            .count();
        let objects: Vec<glib::BoxedAnyObject> = rows[keep..]
            .iter()
            .copied()
            .map(glib::BoxedAnyObject::new)
            .collect();
        if let Some(store) = imp.rows.get() {
            store.splice(keep as u32, store.n_items() - keep as u32, &objects);
        }
        // A header that stayed shows the new count and state.
        let headers: Vec<Option<GridRow>> = imp
            .sections
            .borrow()
            .iter()
            .map(|s| s.header_row.upgrade())
            .collect();
        for (section, row) in headers.into_iter().enumerate() {
            // Not when the row went on to show another section's header.
            if let Some(row) = row.filter(|r| r.imp().section.get() == section) {
                self.update_header(&row);
            }
        }
    }

    fn bind_row(&self, widget: &GridRow, row: Row) {
        let imp = self.imp();
        let columns = imp.columns.get() as usize;
        match row {
            Row::Header(section) => {
                widget.ensure_header(self);
                widget.imp().section.set(section);
                if let Some(s) = imp.sections.borrow().get(section) {
                    s.header_row.set(Some(widget));
                }
                self.update_header(widget);
            }
            Row::Local { start, len } => {
                let Some(model) = imp.local.borrow().clone() else {
                    return;
                };
                let (Some(create), Some(bind), Some(activate)) =
                    (imp.create.get(), imp.bind.get(), imp.activate.get())
                else {
                    return;
                };
                widget.prepare(RowKind::Local, columns, Some(create), activate.clone());
                let items: Vec<Option<glib::Object>> = (0..columns as u32)
                    .map(|i| (i < len).then(|| model.item(start + i)).flatten())
                    .collect();
                widget.fill_local(items, bind);
            }
            Row::Section {
                section,
                start,
                len,
            } => {
                let Some((items, activate)) = imp
                    .sections
                    .borrow()
                    .get(section)
                    .map(|s| (s.items.clone(), s.activate.clone()))
                else {
                    return;
                };
                widget.imp().section.set(section);
                widget.fill_spotify(columns, &items, start as usize, len as usize, activate);
            }
        }
    }

    fn toggle_section(&self, section: usize) {
        let imp = self.imp();
        let key = imp.sections.borrow().get(section).map(|s| s.key.clone());
        if let (Some(settings), Some(key)) = (imp.settings.get(), key) {
            let _ = settings.set_boolean(&key, !settings.boolean(&key));
        }
    }

    /// The header's title and count, open/closed state (shown and told to
    /// screen readers) and the "connect again" note.
    fn update_header(&self, row: &GridRow) {
        let imp = self.imp();
        let row_imp = row.imp();
        let section = row_imp.section.get();
        let open = self.section_open(section);
        let Some((title, count, needs_permission)) = imp
            .sections
            .borrow()
            .get(section)
            .map(|s| (s.title.clone(), s.items.len(), s.needs_permission))
        else {
            return;
        };
        if let Some(content) = row_imp.header_content.borrow().as_ref() {
            content.set_label(&title.replace("{}", &count.to_string()));
            content.set_icon_name(if open {
                "pan-down-symbolic"
            } else {
                "pan-end-symbolic"
            });
        }
        if let Some(toggle) = row_imp.header_toggle.borrow().as_ref() {
            toggle.update_state(&[gtk::accessible::State::Expanded(Some(open))]);
        }
        if let Some(permission) = row_imp.header_permission.borrow().as_ref() {
            permission.set_visible(needs_permission && open);
        }
    }
}

impl Default for SectionedGrid {
    fn default() -> Self {
        glib::Object::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RowKind {
    #[default]
    None,
    Local,
    Header,
    Spotify,
}

mod row_imp {
    use super::*;

    /// A line of tiles (or the section header). It lays them out itself,
    /// each in the middle of an equal column and no wider than it wants
    /// (the cover size), and may get narrower than its tiles: the grid
    /// picks fewer columns then, so the window can shrink.
    #[derive(Default)]
    pub struct GridRow {
        pub kind: Cell<RowKind>,
        /// Each tile is a button around the tile widget.
        pub slots: RefCell<Vec<gtk::Button>>,
        /// The header, in a header row, and its parts that change.
        pub header: RefCell<Option<gtk::Widget>>,
        pub header_toggle: RefCell<Option<gtk::Button>>,
        pub header_content: RefCell<Option<adw::ButtonContent>>,
        pub header_permission: RefCell<Option<gtk::Box>>,
        /// What each library tile shows, for its click.
        pub items: RefCell<Vec<Option<glib::Object>>>,
        pub activate: RefCell<Option<Activate>>,
        /// The grid's column count now (tiles are sized by it, also before
        /// the rows are rebuilt for a new width).
        pub columns: RefCell<Option<Rc<Cell<u32>>>>,
        /// The section of a header or section row.
        pub section: Cell<usize>,
        /// The first section item of the row, and what opens one.
        pub spotify_start: Cell<usize>,
        pub spotify_activate: RefCell<Option<SpotifyActivate>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for GridRow {
        const NAME: &'static str = "JewelcaseGridRow";
        type Type = super::GridRow;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for GridRow {
        fn dispose(&self) {
            self.obj().clear();
        }
    }

    impl GridRow {
        /// As many columns as the grid has now, or as there are tiles
        /// (narrowing: until rebuilt, more tiles than columns share it).
        fn column_count(&self, slots: usize) -> i32 {
            let live = self
                .columns
                .borrow()
                .as_ref()
                .map_or(0, |c| c.get() as usize);
            live.max(slots).max(1) as i32
        }

        /// A tile's width in a column: its natural width (the cover size
        /// and padding), less where the column is narrower, never below
        /// its minimum.
        fn slot_width(slot: &gtk::Button, column: i32) -> i32 {
            let (narrowest, natural, _, _) = slot.measure(gtk::Orientation::Horizontal, -1);
            column.min(natural).max(narrowest)
        }
    }

    impl WidgetImpl for GridRow {
        fn request_mode(&self) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }

        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            if let Some(header) = self.header.borrow().as_ref() {
                let (min, natural, _, _) = header.measure(orientation, for_size);
                return match orientation {
                    gtk::Orientation::Horizontal => (0, natural, -1, -1),
                    _ => (min, natural, -1, -1),
                };
            }
            let slots = self.slots.borrow();
            let count = self.column_count(slots.len());
            match orientation {
                gtk::Orientation::Horizontal => {
                    let natural = slots
                        .iter()
                        .map(|s| s.measure(orientation, -1).1)
                        .max()
                        .unwrap_or(0);
                    (0, natural * count, -1, -1)
                }
                _ => {
                    let (mut min, mut natural) = (0, 0);
                    for slot in slots.iter() {
                        let width = if for_size < 0 {
                            -1
                        } else {
                            Self::slot_width(slot, for_size / count)
                        };
                        let (m, n, _, _) = slot.measure(orientation, width);
                        min = min.max(m);
                        natural = natural.max(n);
                    }
                    (min, natural, -1, -1)
                }
            }
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            if let Some(header) = self.header.borrow().as_ref() {
                header.allocate(width, height, baseline, None);
                return;
            }
            let slots = self.slots.borrow();
            let count = self.column_count(slots.len());
            let column = width / count;
            for (index, slot) in slots.iter().enumerate() {
                // Never below the tile's own minimum (it'd overflow into
                // the next column only while the grid picks fewer columns).
                let slot_width = Self::slot_width(slot, column);
                let x = column * index as i32 + ((column - slot_width) / 2).max(0);
                let transform =
                    gtk::gsk::Transform::new().translate(&gtk::graphene::Point::new(x as f32, 0.0));
                slot.allocate(slot_width, height, -1, Some(transform));
            }
        }
    }
}

glib::wrapper! {
    pub struct GridRow(ObjectSubclass<row_imp::GridRow>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for GridRow {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl GridRow {
    fn clear(&self) {
        let imp = self.imp();
        for slot in imp.slots.take() {
            slot.unparent();
        }
        if let Some(header) = imp.header.take() {
            header.unparent();
        }
        imp.header_toggle.replace(None);
        imp.header_content.replace(None);
        imp.header_permission.replace(None);
    }

    /// `columns` tile buttons of `kind` (reused when they already are).
    fn prepare(&self, kind: RowKind, columns: usize, create: Option<&Create>, activate: Activate) {
        let imp = self.imp();
        imp.activate.replace(Some(activate));
        if imp.kind.get() == kind && imp.slots.borrow().len() == columns {
            return;
        }
        self.clear();
        imp.kind.set(kind);
        self.queue_resize();
        for index in 0..columns {
            let button = gtk::Button::builder()
                .css_classes(["flat", "grid-tile"])
                .build();
            if let Some(create) = create {
                button.set_child(Some(&create()));
            }
            // Spotify's tiles: their names in full, only when cut off
            // (library tiles have their own).
            super::tooltip_when_cut(&button);
            button.connect_clicked(glib::clone!(
                #[weak(rename_to = row)]
                self,
                move |_| row.clicked(index)
            ));
            button.set_parent(self);
            imp.slots.borrow_mut().push(button);
        }
    }

    fn clicked(&self, index: usize) {
        let imp = self.imp();
        match imp.kind.get() {
            RowKind::Local => {
                let item = imp.items.borrow().get(index).cloned().flatten();
                let activate = imp.activate.borrow().clone();
                if let (Some(item), Some(activate)) = (item, activate) {
                    activate(&item);
                }
            }
            RowKind::Spotify => {
                let activate = imp.spotify_activate.borrow().clone();
                if let Some(activate) = activate {
                    activate(imp.spotify_start.get() + index);
                }
            }
            _ => {}
        }
    }

    fn fill_local(&self, items: Vec<Option<glib::Object>>, bind: &Bind) {
        let imp = self.imp();
        for (button, item) in imp.slots.borrow().iter().zip(&items) {
            // Empty places at the end: hidden (not announced), the row's
            // layout keeps the tiles their width.
            button.set_visible(item.is_some());
            if let (Some(item), Some(child)) = (item, button.child()) {
                bind(&child, item);
            }
        }
        imp.items.replace(items);
    }

    fn fill_spotify(
        &self,
        columns: usize,
        items: &[ShelfItem],
        start: usize,
        len: usize,
        activate: Option<SpotifyActivate>,
    ) {
        let imp = self.imp();
        self.prepare(RowKind::Spotify, columns, None, Rc::new(|_| {}));
        imp.spotify_start.set(start);
        imp.spotify_activate.replace(activate);
        for (index, button) in imp.slots.borrow().iter().enumerate() {
            let item = (index < len).then(|| items.get(start + index)).flatten();
            button.set_visible(item.is_some());
            match item {
                Some(item) => {
                    button.set_child(Some(&tile_content(item).0));
                }
                None => button.set_child(None::<&gtk::Widget>),
            }
        }
    }

    /// A section's header (built once per row widget, then only updated,
    /// so the toggle keeps the focus): the open/close toggle and, when the
    /// login lacks the permission, a way to connect again.
    fn ensure_header(&self, grid: &SectionedGrid) {
        let imp = self.imp();
        if imp.kind.get() == RowKind::Header && imp.header.borrow().is_some() {
            return;
        }
        self.clear();
        imp.kind.set(RowKind::Header);
        let header = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .css_classes(["grid-section-header"])
            .build();
        let content = adw::ButtonContent::new();
        let toggle = gtk::Button::builder()
            .child(&content)
            .halign(gtk::Align::Start)
            .css_classes(["flat", "heading"])
            .build();
        toggle.connect_clicked(glib::clone!(
            #[weak]
            grid,
            #[weak(rename_to = row)]
            self,
            move |_| grid.toggle_section(row.imp().section.get())
        ));
        header.append(&toggle);
        let permission = gtk::Box::builder().spacing(12).margin_start(12).build();
        permission.append(
            &gtk::Label::builder()
                .label(gettext(
                    "Connect to Spotify again to allow Jewelcase to read your saved albums and followed artists.",
                ))
                .wrap(true)
                .xalign(0.0)
                .hexpand(true)
                .css_classes(["dim-label"])
                .build(),
        );
        let button = gtk::Button::builder()
            .label(gettext("_Connect Again"))
            .use_underline(true)
            .valign(gtk::Align::Center)
            .css_classes(["pill"])
            .build();
        button.connect_clicked(|_| {
            let spotify = JewelcaseApplication::get().spotify();
            glib::spawn_future_local(async move { spotify.connect().await });
        });
        permission.append(&button);
        header.append(&permission);
        header.set_parent(self);
        imp.header.replace(Some(header.upcast()));
        imp.header_toggle.replace(Some(toggle));
        imp.header_content.replace(Some(content));
        imp.header_permission.replace(Some(permission));
        self.queue_resize();
    }
}

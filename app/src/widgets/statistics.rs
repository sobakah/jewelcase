//! Listening statistics (CONCEPT.md §6.3): what played most this week,
//! month, year or ever, with listening time over the period and how
//! each entry moved against the period before.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use adw::prelude::*;
use gatefold_core::db::{PlayEntry, Ranked, Summary, Trend};
use gatefold_core::library::covers::CoverSize;
use gettextrs::{gettext, ngettext};
use gtk::{gio, glib};

use super::CoverImage;
use crate::application::GatefoldApplication;
use crate::config::APP_ID;
use crate::covers;
use crate::format;
use crate::objects::TrackObject;

const TOP_SONGS: usize = 20;
const TOP_ARTISTS: usize = 8;
const TOP_ALBUMS: usize = 8;
const TOP_GENRES: usize = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Period {
    Week,
    Month,
    Year,
    All,
}

impl Period {
    fn from_name(name: &str) -> Self {
        match name {
            "week" => Period::Week,
            "year" => Period::Year,
            "all" => Period::All,
            _ => Period::Month,
        }
    }

    /// "Up 3 places since the week before".
    fn since(self) -> String {
        match self {
            Period::Week => gettext("since the week before"),
            Period::Month => gettext("since the month before"),
            Period::Year => gettext("since the year before"),
            Period::All => String::new(),
        }
    }
}

/// A stretch of the timeline: start, end (Unix seconds), its label under
/// the bars (may be empty) and its name in the tooltip.
struct Bucket {
    start: i64,
    end: i64,
    label: String,
    name: String,
}

/// The period's buckets, oldest first; the first starts the period and
/// the period before is as long.
fn buckets(period: Period, now: &glib::DateTime, first_play: Option<i64>) -> Vec<Bucket> {
    let today = glib::DateTime::from_local(now.year(), now.month(), now.day_of_month(), 0, 0, 0.0)
        .unwrap_or_else(|_| now.clone());
    let days = |count: i32, label: &dyn Fn(&glib::DateTime, i32) -> String| {
        (0..count)
            .map(|i| {
                let start = today
                    .add_days(i - count + 1)
                    .unwrap_or_else(|_| today.clone());
                let end = start.add_days(1).unwrap_or_else(|_| start.clone());
                Bucket {
                    start: start.to_unix(),
                    end: end.to_unix(),
                    label: label(&start, i),
                    name: start
                        .format("%A, %e %B")
                        .map(Into::into)
                        .unwrap_or_default(),
                }
            })
            .collect()
    };
    let month_start = glib::DateTime::from_local(now.year(), now.month(), 1, 0, 0, 0.0)
        .unwrap_or_else(|_| today.clone());
    let months = |count: i32, label: &dyn Fn(&glib::DateTime, i32) -> String| {
        (0..count)
            .map(|i| {
                let start = month_start
                    .add_months(i - count + 1)
                    .unwrap_or_else(|_| month_start.clone());
                let end = start.add_months(1).unwrap_or_else(|_| start.clone());
                Bucket {
                    start: start.to_unix(),
                    end: end.to_unix(),
                    label: label(&start, i),
                    name: start.format("%B %Y").map(Into::into).unwrap_or_default(),
                }
            })
            .collect()
    };
    match period {
        Period::Week => days(7, &|day, _| {
            day.format("%a").map(Into::into).unwrap_or_default()
        }),
        // Every seventh day named.
        Period::Month => days(30, &|day, i| {
            if i % 7 == 0 {
                day.format("%e %b")
                    .map(|s| s.trim().to_owned())
                    .unwrap_or_default()
            } else {
                String::new()
            }
        }),
        Period::Year => months(12, &|month, _| {
            month.format("%b").map(Into::into).unwrap_or_default()
        }),
        Period::All => {
            let first = first_play
                .and_then(|t| glib::DateTime::from_unix_local(t).ok())
                .unwrap_or_else(|| now.clone());
            let count = (now.year() - first.year()) * 12 + now.month() - first.month() + 1;
            if count <= 36 {
                let count = count.max(1);
                months(count, &|month, i| {
                    let every = (count + 5) / 6;
                    if i % every == 0 {
                        month.format("%b %y").map(Into::into).unwrap_or_default()
                    } else {
                        String::new()
                    }
                })
            } else {
                // Years instead.
                (first.year()..=now.year())
                    .map(|year| {
                        let start = glib::DateTime::from_local(year, 1, 1, 0, 0, 0.0)
                            .map_or(0, |d| d.to_unix());
                        let end = glib::DateTime::from_local(year + 1, 1, 1, 0, 0, 0.0)
                            .map_or(i64::MAX, |d| d.to_unix());
                        Bucket {
                            start,
                            end,
                            label: year.to_string(),
                            name: year.to_string(),
                        }
                    })
                    .collect()
            }
        }
    }
}

/// Opens the statistics.
pub fn show(parent: Option<&gtk::Window>) {
    let settings = gio::Settings::new(APP_ID);
    let dialog = adw::Dialog::builder()
        .title(gettext("Listening Statistics"))
        .content_width(680)
        .content_height(820)
        .build();
    let periods = adw::ToggleGroup::new();
    for (name, label) in [
        ("week", gettext("Week")),
        ("month", gettext("Month")),
        ("year", gettext("Year")),
        ("all", gettext("All Time")),
    ] {
        periods.add(adw::Toggle::builder().name(name).label(label).build());
    }
    settings
        .bind("statistics-period", &periods, "active-name")
        .build();
    let header = adw::HeaderBar::new();
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    let switcher = gtk::Box::builder()
        .halign(gtk::Align::Center)
        .margin_top(6)
        .margin_bottom(6)
        .build();
    switcher.append(&periods);
    toolbar.add_top_bar(&switcher);
    let content = adw::Bin::new();
    toolbar.set_content(Some(&content));
    dialog.set_child(Some(&toolbar));

    let generation = Rc::new(std::cell::Cell::new(0u64));
    let refresh = Rc::new(glib::clone!(
        #[weak]
        dialog,
        #[weak]
        content,
        #[weak]
        periods,
        #[strong]
        generation,
        move || {
            let period = Period::from_name(periods.active_name().as_deref().unwrap_or("month"));
            // First, so a count still running for another period is
            // dropped also when this one shows a status page.
            let this = generation.get() + 1;
            generation.set(this);
            let span = match span(period) {
                Ok(span) => span,
                Err(status) => {
                    content.set_child(Some(&status));
                    return;
                }
            };
            // Reading and counting take a moment with years of plays: on
            // a worker thread, the page waits with a spinner.
            let spinner = adw::Spinner::builder()
                .width_request(32)
                .height_request(32)
                .halign(gtk::Align::Center)
                .valign(gtk::Align::Center)
                .vexpand(true)
                .build();
            content.set_child(Some(&spinner));
            glib::spawn_future_local(glib::clone!(
                #[weak]
                dialog,
                #[weak]
                content,
                #[strong]
                generation,
                async move {
                    let Span { buckets, from, to } = span;
                    let counted = gio::spawn_blocking(move || {
                        let loaded = load(period, from, to)?;
                        let summary =
                            Summary::new(&loaded.current, &loaded.previous, &loaded.known);
                        Some((loaded.current, summary))
                    })
                    .await;
                    let Ok(counted) = counted else {
                        return;
                    };
                    // Another period was chosen meanwhile.
                    if generation.get() != this {
                        return;
                    }
                    let Some((current, summary)) = counted else {
                        content.set_child(Some(&nothing_played()));
                        return;
                    };
                    content.set_child(Some(&build(&dialog, period, &buckets, &current, &summary)));
                }
            ));
        }
    ));
    periods.connect_active_name_notify(glib::clone!(
        #[strong]
        refresh,
        move |_| refresh()
    ));
    refresh();
    dialog.present(parent);
}

/// A period's plays, ready to count.
struct Loaded {
    current: Vec<PlayEntry>,
    previous: Vec<PlayEntry>,
    /// Artist names that stay whole ("Mumford & Sons").
    known: HashSet<String>,
}

/// The stretch of time a period covers.
struct Span {
    buckets: Vec<Bucket>,
    from: i64,
    to: i64,
}

/// The period's time, or the page saying there's nothing logged yet.
fn span(period: Period) -> Result<Span, gtk::Widget> {
    let app = GatefoldApplication::get();
    let library = app.library();
    let now = glib::DateTime::now_local().unwrap_or_else(|_| glib::DateTime::now_utc().unwrap());
    let (first_play, logging) = (
        library.db().and_then(|db| db.first_play().ok().flatten()),
        gio::Settings::new(APP_ID).boolean("listening-stats"),
    );
    let Some(first_play) = first_play else {
        let status = adw::StatusPage::builder()
            .icon_name("audio-x-generic-symbolic")
            .title(gettext("No Listening Statistics Yet"))
            .description(if logging {
                gettext("Songs you listen to show up here once they’ve played for half their length or 30 seconds")
            } else {
                gettext("Turned off in Preferences → Playback")
            })
            .vexpand(true)
            .build();
        return Err(status.upcast());
    };
    let buckets = buckets(period, &now, Some(first_play));
    let from = buckets.first().map_or(0, |b| b.start);
    let to = buckets.last().map_or(i64::MAX, |b| b.end);
    Ok(Span { buckets, from, to })
}

/// The plays from `from` to `to` (and as long before), on a worker
/// thread with its own connection; `None` when there are none.
fn load(period: Period, from: i64, to: i64) -> Option<Loaded> {
    let db = match gatefold_core::db::Database::open(&crate::config::database_path()) {
        Ok(db) => db,
        Err(err) => {
            tracing::warn!("Cannot read the play log: {err}");
            return None;
        }
    };
    let current = db.play_log(from, to).unwrap_or_default();
    if current.is_empty() {
        return None;
    }
    let previous = if period == Period::All {
        Vec::new()
    } else {
        db.play_log(from - (to - from), from).unwrap_or_default()
    };
    let known = db.known_artist_names().unwrap_or_default();
    Some(Loaded {
        current,
        previous,
        known,
    })
}

fn nothing_played() -> gtk::Widget {
    adw::StatusPage::builder()
        .icon_name("audio-x-generic-symbolic")
        .title(gettext("Nothing Played in This Time"))
        .description(gettext("Choose a longer time above"))
        .vexpand(true)
        .build()
        .upcast()
}

/// The page for one period, once counted.
fn build(
    dialog: &adw::Dialog,
    period: Period,
    buckets: &[Bucket],
    current: &[PlayEntry],
    summary: &Summary,
) -> gtk::Widget {
    let column = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(30)
        .margin_top(12)
        .margin_bottom(24)
        .margin_start(12)
        .margin_end(12)
        .build();
    column.append(&totals(summary));
    column.append(&timeline(buckets, current));
    let since = period.since();
    if !summary.artists.is_empty() {
        column.append(&section(
            &gettext("Top Artists"),
            None,
            &artist_tiles(
                dialog,
                &summary.artists[..summary.artists.len().min(TOP_ARTISTS)],
                &since,
            ),
        ));
    }
    if !summary.albums.is_empty() {
        column.append(&section(
            &gettext("Top Albums"),
            None,
            &album_tiles(
                dialog,
                &summary.albums[..summary.albums.len().min(TOP_ALBUMS)],
                &since,
            ),
        ));
    }
    let songs = &summary.songs[..summary.songs.len().min(TOP_SONGS)];
    let play_all = gtk::Button::builder()
        .icon_name("media-playback-start-symbolic")
        .tooltip_text(gettext("Play These Songs"))
        .valign(gtk::Align::Center)
        .css_classes(["flat", "circular"])
        .build();
    let ids: Vec<i64> = songs.iter().map(|s| s.track_id).collect();
    let context = match period {
        Period::Week => gettext("Top Songs of the Week"),
        Period::Month => gettext("Top Songs of the Month"),
        Period::Year => gettext("Top Songs of the Year"),
        Period::All => gettext("Top Songs"),
    };
    play_all.connect_clicked(glib::clone!(
        #[weak]
        dialog,
        move |_| {
            let app = GatefoldApplication::get();
            let tracks = app.library().tracks(&ids);
            let objects: Vec<TrackObject> = ids
                .iter()
                .filter_map(|id| tracks.get(id).cloned().map(TrackObject::new))
                .collect();
            app.player()
                .play_tracks(objects, Some(0), false, Some(context.clone()));
            dialog.close();
        }
    ));
    column.append(&section(
        &gettext("Top Songs"),
        Some(&play_all),
        &song_list(songs, &since),
    ));
    if !summary.genres.is_empty() {
        column.append(&section(
            &gettext("Top Genres"),
            None,
            &genre_list(&summary.genres[..summary.genres.len().min(TOP_GENRES)]),
        ));
    }
    let clamp = adw::Clamp::builder()
        .maximum_size(640)
        .child(&column)
        .build();
    gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&clamp)
        .vexpand(true)
        .build()
        .upcast()
}

/// Listening time, plays and artists, as big numbers.
fn totals(summary: &Summary) -> gtk::Widget {
    let row = gtk::Box::builder().spacing(12).homogeneous(true).build();
    let artists = summary.artists.len() as u32;
    let songs = summary.songs.len() as u32;
    for (value, caption) in [
        (format::length(summary.ms_played), gettext("Listened")),
        (
            summary.plays.to_string(),
            ngettext("Play", "Plays", summary.plays),
        ),
        (songs.to_string(), ngettext("Song", "Songs", songs)),
        (artists.to_string(), ngettext("Artist", "Artists", artists)),
    ] {
        let card = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(3)
            .css_classes(["card", "stats-total"])
            .build();
        card.append(
            &gtk::Label::builder()
                .label(value)
                .wrap(true)
                .justify(gtk::Justification::Center)
                .css_classes(["title-2"])
                .build(),
        );
        card.append(
            &gtk::Label::builder()
                .label(caption)
                .css_classes(["caption", "dim-label"])
                .build(),
        );
        row.append(&card);
    }
    row.upcast()
}

/// Listening time per day (or month, or year) as bars.
fn timeline(buckets: &[Bucket], plays: &[PlayEntry]) -> gtk::Widget {
    let mut totals = vec![0u64; buckets.len()];
    for play in plays {
        if let Some(i) = buckets
            .iter()
            .position(|b| play.played >= b.start && play.played < b.end)
        {
            totals[i] += play.ms_played;
        }
    }
    let names: Vec<String> = buckets.iter().map(|b| b.name.clone()).collect();
    let totals = Rc::new(totals);
    let area = gtk::DrawingArea::builder()
        .content_height(120)
        .hexpand(true)
        .has_tooltip(true)
        .css_classes(["stats-bars"])
        .build();
    area.update_property(&[gtk::accessible::Property::Label(&gettext(
        "Listening time over this period",
    ))]);
    area.set_draw_func(glib::clone!(
        #[strong]
        totals,
        move |area, cr, width, height| {
            let max = totals.iter().copied().max().unwrap_or(0).max(1) as f64;
            let n = totals.len().max(1) as f64;
            let slot = width as f64 / n;
            let gap = (slot * 0.25).clamp(1.0, 8.0);
            let bar = (slot - gap).max(1.0);
            let color = area.color();
            for (i, &total) in totals.iter().enumerate() {
                let x = i as f64 * slot + gap / 2.0;
                let h = if total == 0 {
                    2.0
                } else {
                    (total as f64 / max * (height as f64 - 4.0)).max(4.0)
                };
                let y = height as f64 - h;
                let alpha = if total == 0 { 0.15 } else { 1.0 };
                cr.set_source_rgba(
                    color.red() as f64,
                    color.green() as f64,
                    color.blue() as f64,
                    color.alpha() as f64 * alpha,
                );
                let r = (bar / 2.0).min(4.0).min(h / 2.0);
                cr.new_sub_path();
                cr.arc(x + bar - r, y + r, r, -std::f64::consts::FRAC_PI_2, 0.0);
                cr.line_to(x + bar, y + h);
                cr.line_to(x, y + h);
                cr.arc(
                    x + r,
                    y + r,
                    r,
                    std::f64::consts::PI,
                    1.5 * std::f64::consts::PI,
                );
                cr.close_path();
                let _ = cr.fill();
            }
        }
    ));
    area.connect_query_tooltip(glib::clone!(
        #[strong]
        totals,
        move |area, x, _, _, tooltip| {
            let width = area.width().max(1);
            let i = (x.max(0) as usize * totals.len() / width as usize).min(totals.len() - 1);
            let text = if totals[i] == 0 {
                // Translators: a day or month, then that nothing played.
                gettext("{}: nothing played").replace("{}", &names[i])
            } else {
                format!("{}: {}", names[i], format::length(totals[i]))
            };
            tooltip.set_text(Some(&text));
            true
        }
    ));
    // Each label spans the bars up to the next one (one column per bar),
    // so it has room without widening the dialog; a last one with too
    // little room is left out.
    let labels = gtk::Grid::builder().column_homogeneous(true).build();
    let starts: Vec<usize> = (0..buckets.len())
        .filter(|&i| !buckets[i].label.is_empty())
        .collect();
    let first_span = match starts.as_slice() {
        [a, b, ..] => b - a,
        _ => buckets.len(),
    };
    for (n, &i) in starts.iter().enumerate() {
        let span = starts.get(n + 1).unwrap_or(&buckets.len()) - i;
        if span * 2 < first_span {
            continue;
        }
        labels.attach(
            &gtk::Label::builder()
                .label(&buckets[i].label)
                .css_classes(["caption", "dim-label"])
                .halign(gtk::Align::Start)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .build(),
            i as i32,
            0,
            span as i32,
            1,
        );
    }
    // Every bar has its column, also without labels.
    for i in 0..buckets.len() {
        if !starts.contains(&i) {
            labels.attach(
                &gtk::Box::new(gtk::Orientation::Horizontal, 0),
                i as i32,
                1,
                1,
                1,
            );
        }
    }
    let card = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .css_classes(["card", "stats-timeline"])
        .build();
    card.append(&area);
    card.append(&labels);
    card.upcast()
}

/// A heading (with an optional button) above `body`.
fn section(title: &str, button: Option<&gtk::Button>, body: &impl IsA<gtk::Widget>) -> gtk::Widget {
    let column = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .build();
    let header = gtk::Box::builder().spacing(6).build();
    header.append(
        &gtk::Label::builder()
            .label(title)
            .halign(gtk::Align::Start)
            .hexpand(true)
            .css_classes(["heading"])
            .accessible_role(gtk::AccessibleRole::Heading)
            .build(),
    );
    if let Some(button) = button {
        header.append(button);
    }
    column.append(&header);
    column.append(body);
    column.upcast()
}

/// "12 plays · 48 min"
fn plays_line(ranked: &Ranked) -> String {
    format!(
        "{} · {}",
        ngettext("{} play", "{} plays", ranked.plays).replace("{}", &ranked.plays.to_string()),
        format::length(ranked.ms_played)
    )
}

/// ↑3, ↓2 or "New", with what it means as tooltip; nothing without a
/// change or a period before.
fn trend_badge(trend: Option<Trend>, since: &str) -> Option<gtk::Label> {
    let (text, class, tooltip) = match trend? {
        Trend::New => (
            gettext("New"),
            "accent",
            // Translators: e.g. "Not played since the week before".
            gettext("Not played {}").replace("{}", since),
        ),
        Trend::Moved(0) => return None,
        Trend::Moved(up) if up > 0 => (
            format!("↑{up}"),
            "success",
            ngettext("Up {n} place {since}", "Up {n} places {since}", up as u32)
                .replace("{n}", &up.to_string())
                .replace("{since}", since),
        ),
        Trend::Moved(down) => (
            format!("↓{}", -down),
            "error",
            ngettext(
                "Down {n} place {since}",
                "Down {n} places {since}",
                (-down) as u32,
            )
            .replace("{n}", &(-down).to_string())
            .replace("{since}", since),
        ),
    };
    Some(
        gtk::Label::builder()
            .label(text)
            .tooltip_text(tooltip)
            .valign(gtk::Align::Center)
            .css_classes(["caption-heading", "stats-trend", class])
            .build(),
    )
}

fn flow_box() -> gtk::FlowBox {
    gtk::FlowBox::builder()
        .homogeneous(true)
        .min_children_per_line(2)
        .max_children_per_line(4)
        .selection_mode(gtk::SelectionMode::None)
        .row_spacing(6)
        .column_spacing(6)
        .css_classes(["stats-flow"])
        .build()
}

/// A tile button: picture, two lines and the trend; opens `action` (a
/// library album or artist) if there is one.
fn tile(
    dialog: &adw::Dialog,
    picture: &impl IsA<gtk::Widget>,
    title: &str,
    subtitle: &str,
    trend: Option<gtk::Label>,
    action: Option<(&'static str, i64)>,
) -> gtk::Widget {
    let column = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .build();
    column.append(picture);
    let title_row = gtk::Box::builder()
        .spacing(6)
        .halign(gtk::Align::Center)
        .build();
    title_row.append(
        &gtk::Label::builder()
            .label(title)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .max_width_chars(16)
            .css_classes(["heading"])
            .build(),
    );
    if let Some(trend) = trend {
        title_row.append(&trend);
    }
    column.append(&title_row);
    column.append(
        &gtk::Label::builder()
            .label(subtitle)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .max_width_chars(20)
            .css_classes(["caption", "dim-label"])
            .build(),
    );
    // Only in the statistics (not in the library): nothing to open.
    let Some((name, id)) = action else {
        column.add_css_class("grid-tile");
        super::tooltip_when_cut(&column);
        return column.upcast();
    };
    let button = gtk::Button::builder()
        .child(&column)
        .css_classes(["flat", "grid-tile"])
        .build();
    super::tooltip_when_cut(&button);
    {
        button.connect_clicked(glib::clone!(
            #[weak]
            dialog,
            move |button| {
                let _ = button.activate_action(name, Some(&id.to_variant()));
                dialog.close();
            }
        ));
    }
    button.upcast()
}

fn artist_tiles(dialog: &adw::Dialog, artists: &[Ranked], since: &str) -> gtk::Widget {
    let library = GatefoldApplication::get().library();
    let flow = flow_box();
    for ranked in artists {
        let id = library
            .db()
            .and_then(|db| db.artist_named(&ranked.name).ok().flatten());
        let portrait = id
            .and_then(|id| library.artist(id))
            .and_then(|artist| artist.artist().portrait_key.clone())
            .and_then(|key| covers::portrait(&key, CoverSize::Grid));
        let avatar = adw::Avatar::new(96, Some(&ranked.name), true);
        avatar.set_custom_image(portrait.as_ref());
        avatar.set_halign(gtk::Align::Center);
        flow.append(&tile(
            dialog,
            &avatar,
            &ranked.name,
            &plays_line(ranked),
            trend_badge(ranked.trend, since),
            id.map(|id| ("win.show-artist", id)),
        ));
    }
    flow.upcast()
}

fn album_tiles(dialog: &adw::Dialog, albums: &[Ranked], since: &str) -> gtk::Widget {
    let library = GatefoldApplication::get().library();
    let ids: Vec<i64> = albums.iter().map(|a| a.track_id).collect();
    let tracks = library.tracks(&ids);
    let flow = flow_box();
    for ranked in albums {
        let found = library.db().and_then(|db| {
            db.album_named(&ranked.name, ranked.artist.as_deref())
                .ok()
                .flatten()
        });
        let cover_key = found.as_ref().and_then(|(_, key)| key.clone()).or_else(|| {
            tracks
                .get(&ranked.track_id)
                .and_then(|t| t.album_cover_key.clone().or(t.record.cover_key.clone()))
        });
        let cover: CoverImage = glib::Object::builder()
            .property("min-size", 112)
            .property("natural-size", 112)
            .build();
        cover.set_cover(cover_key.as_deref());
        cover.set_label(&ranked.name);
        cover.set_halign(gtk::Align::Center);
        let subtitle = match &ranked.artist {
            Some(artist) => format!("{artist} · {}", short_plays(ranked.plays)),
            None => short_plays(ranked.plays),
        };
        flow.append(&tile(
            dialog,
            &cover,
            &ranked.name,
            &subtitle,
            trend_badge(ranked.trend, since),
            found.map(|(id, _)| ("win.show-album", id)),
        ));
    }
    flow.upcast()
}

fn short_plays(plays: u32) -> String {
    ngettext("{} play", "{} plays", plays).replace("{}", &plays.to_string())
}

fn song_list(songs: &[Ranked], since: &str) -> gtk::Widget {
    let library = GatefoldApplication::get().library();
    let ids: Vec<i64> = songs.iter().map(|s| s.track_id).collect();
    let tracks = Rc::new(RefCell::new(library.tracks(&ids)));
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["boxed-list"])
        .build();
    let mut rows: HashMap<i32, i64> = HashMap::new();
    for (place, ranked) in songs.iter().enumerate() {
        let track = tracks.borrow().get(&ranked.track_id).cloned();
        let row = adw::ActionRow::builder()
            .title(glib::markup_escape_text(&ranked.name))
            .subtitle(glib::markup_escape_text(&match &ranked.artist {
                Some(artist) => format!("{artist} · {}", plays_line(ranked)),
                None => plays_line(ranked),
            }))
            .activatable(track.is_some())
            .build();
        let rank = gtk::Label::builder()
            .label((place + 1).to_string())
            .width_chars(2)
            .xalign(1.0)
            .css_classes(["numeric", "dim-label", "heading"])
            .build();
        let cover: CoverImage = glib::Object::builder()
            .property("min-size", 40)
            .property("natural-size", 40)
            .build();
        // As everywhere: the album's chosen cover first, else the file's.
        cover.set_cover(track.as_ref().and_then(|t| {
            t.album_cover_key
                .as_deref()
                .or(t.record.cover_key.as_deref())
        }));
        cover.set_valign(gtk::Align::Center);
        // Prefixes are packed from the start: the rank first.
        row.add_prefix(&rank);
        row.add_prefix(&cover);
        if let Some(badge) = trend_badge(ranked.trend, since) {
            row.add_suffix(&badge);
        }
        rows.insert(place as i32, ranked.track_id);
        list.append(&row);
    }
    list.connect_row_activated(move |list, row| {
        let Some(&id) = rows.get(&row.index()) else {
            return;
        };
        let Some(track) = tracks.borrow().get(&id).cloned() else {
            return;
        };
        GatefoldApplication::get()
            .player()
            .play_now(TrackObject::new(track));
        if let Some(dialog) = list
            .ancestor(adw::Dialog::static_type())
            .and_downcast::<adw::Dialog>()
        {
            dialog.close();
        }
    });
    list.upcast()
}

/// Genres with a bar of their share against the top one.
fn genre_list(genres: &[Ranked]) -> gtk::Widget {
    let top = genres.first().map_or(1, |g| g.plays.max(1));
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["boxed-list"])
        .build();
    for genre in genres {
        let row = adw::ActionRow::builder()
            .title(glib::markup_escape_text(&genre.name))
            .subtitle(plays_line(genre))
            .build();
        let level = gtk::LevelBar::builder()
            .min_value(0.0)
            .max_value(1.0)
            .value(genre.plays as f64 / top as f64)
            .valign(gtk::Align::Center)
            .width_request(140)
            .build();
        // One color, not the low/high/full steps.
        level.remove_offset_value(Some(gtk::LEVEL_BAR_OFFSET_LOW));
        level.remove_offset_value(Some(gtk::LEVEL_BAR_OFFSET_HIGH));
        level.remove_offset_value(Some(gtk::LEVEL_BAR_OFFSET_FULL));
        row.add_suffix(&level);
        list.append(&row);
    }
    list.upcast()
}

//! Settings window: the key, the languages and words to expect, the
//! microphone, and how dictation behaves.
//!
//! Laid out the way a Mac app's settings are: a toolbar of tabs along the top,
//! and under each a few rounded groups, the name of a setting on the left and
//! its control on the right, with a line of explanation under the group rather
//! than wedged between the controls.
//!
//! The key lives in `~/Library/Application Support/voi/config.env` and the rest
//! in `settings.json` beside it — never in the bundle. That is what makes the
//! app handable to someone else: they get the same binary and type their own
//! key on first run, rather than inheriting yours or needing a rebuild.
//!
//! Everything here is saved to disk and read back when the window opens. A
//! control that only changes the running session is a bug, not a feature.
//! Launch at login is the one setting kept by macOS rather than by us — see
//! [`crate::mac::login`] — and it is read back from macOS the same way.
//!
//! There is no Save button. A window of preferences with a button that has to
//! be pressed afterwards is a window that lies to you: it shows the setting you
//! chose while running on the one you had. Instead the panel is read a couple of
//! times a second while it is open, and written whenever what it says differs
//! from what is on disk — see [`Settings::autosave`]. The one exception is the
//! API key, which is left alone while the cursor is still in it, because half a
//! key saved is a key that does not work.

use std::cell::{Cell, RefCell};

use objc2::rc::Retained;
use objc2::{msg_send, MainThreadOnly};
use objc2_app_kit::{
    NSBackingStoreType, NSBox, NSBoxType, NSButton, NSButtonType, NSCellImagePosition, NSColor,
    NSControlStateValueOff, NSControlStateValueOn, NSFont, NSFontWeightSemibold, NSImage,
    NSLineBreakMode, NSPopUpButton, NSScrollView, NSSecureTextField, NSSlider, NSSwitch,
    NSTabViewController, NSTabViewControllerTabStyle, NSTabViewItem, NSTextAlignment,
    NSTextField, NSTextView, NSTitlePosition, NSTokenField, NSView, NSViewController, NSWindow,
    NSWindowOrderingMode, NSWindowStyleMask, NSWindowToolbarStyle,
};
use objc2_foundation::{ns_string, MainThreadMarker, NSPoint, NSRect, NSSize, NSString};
use voi_core::prefs::{self, DictationKey, DictationMode, Prefs};

/// Every tab is the same size, so switching between them never makes the
/// window jump.
const WIDTH: f64 = 560.0;
const HEIGHT: f64 = 500.0;
/// Space between the window's edge and the groups.
const PAD: f64 = 20.0;
/// Width of a group.
const GROUP: f64 = WIDTH - 2.0 * PAD;
/// Inside a group: from its edge to a row's label, and to a row's control.
const INSET: f64 = 16.0;
const INSET_RIGHT: f64 = 14.0;
/// Height of an ordinary row.
const ROW: f64 = 44.0;
/// Width of the wide controls on the right of a row.
const FIELD: f64 = 290.0;
/// What the footer at the bottom of every tab takes.
const FOOTER: f64 = 44.0;

/// One tab's worth of view, laid out downward from the top edge.
///
/// AppKit measures from the bottom left, so `y` is the top of the next thing to
/// place and every frame is written as `y - height`.
struct Page {
    mtm: MainThreadMarker,
    view: Retained<NSView>,
    y: f64,
    /// The background of the group being filled, and where it began, until
    /// [`Page::end_group`] knows how tall it is.
    group: Option<(Retained<NSBox>, f64)>,
    rows: usize,
}

impl Page {
    fn new(mtm: MainThreadMarker) -> Self {
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(WIDTH, HEIGHT));
        let view: Retained<NSView> = unsafe { msg_send![NSView::alloc(mtm), initWithFrame: frame] };
        Self { mtm, view, y: HEIGHT - PAD, group: None, rows: 0 }
    }

    fn add(&self, view: &NSView) {
        self.view.addSubview(view);
    }

    /// The name of a group, above it.
    fn header(&mut self, text: &str) {
        let label = NSTextField::labelWithString(&NSString::from_str(text), self.mtm);
        label.setFont(Some(&NSFont::systemFontOfSize_weight(13.0, unsafe { NSFontWeightSemibold })));
        label.setFrame(NSRect::new(NSPoint::new(PAD + 4.0, self.y - 17.0), NSSize::new(GROUP, 17.0)));
        self.add(&label);
        self.y -= 17.0 + 8.0;
    }

    /// Starts a rounded group. Its rows follow; [`Page::end_group`] closes it.
    fn begin_group(&mut self) {
        let background: Retained<NSBox> = unsafe {
            msg_send![NSBox::alloc(self.mtm), initWithFrame: NSRect::ZERO]
        };
        background.setBoxType(NSBoxType::Custom);
        background.setTitlePosition(NSTitlePosition::NoTitle);
        background.setCornerRadius(10.0);
        background.setBorderWidth(0.5);
        background.setBorderColor(&NSColor::separatorColor());
        background.setFillColor(&NSColor::quinarySystemFillColor());
        // Behind whatever the rows put on top of it.
        self.view.addSubview_positioned_relativeTo(&background, NSWindowOrderingMode::Below, None);
        self.group = Some((background, self.y));
        self.rows = 0;
    }

    /// A row in the open group, with `label` on the left. Returns the middle
    /// of the row, for lining the control up with the label.
    fn row(&mut self, label: &str, height: f64) -> f64 {
        if self.rows > 0 {
            // A hairline between rows, indented to where the labels start.
            let line: Retained<NSBox> = unsafe {
                msg_send![
                    NSBox::alloc(self.mtm),
                    initWithFrame: NSRect::new(
                        NSPoint::new(PAD + INSET, self.y - 0.5),
                        NSSize::new(GROUP - INSET, 1.0)
                    )
                ]
            };
            line.setBoxType(NSBoxType::Separator);
            self.add(&line);
        }
        self.rows += 1;
        let middle = self.y - height / 2.0;
        let text = NSTextField::labelWithString(&NSString::from_str(label), self.mtm);
        text.setFont(Some(&NSFont::systemFontOfSize(13.0)));
        text.setFrame(NSRect::new(
            NSPoint::new(PAD + INSET, middle - 8.5),
            NSSize::new(GROUP - INSET - FIELD - INSET_RIGHT - 8.0, 17.0),
        ));
        self.add(&text);
        self.y -= height;
        middle
    }

    fn end_group(&mut self) {
        if let Some((background, top)) = self.group.take() {
            background.setFrame(NSRect::new(
                NSPoint::new(PAD, self.y),
                NSSize::new(GROUP, top - self.y),
            ));
        }
        self.y -= 6.0;
    }

    /// Where a control `size` big goes, right-aligned in a row whose middle is
    /// `middle`.
    fn right(size: NSSize, middle: f64) -> NSRect {
        NSRect::new(
            NSPoint::new(PAD + GROUP - INSET_RIGHT - size.width, middle - size.height / 2.0),
            size,
        )
    }

    /// A quiet line or two of explanation, under a group.
    fn hint(&mut self, text: &str) {
        let label = NSTextField::wrappingLabelWithString(&NSString::from_str(text), self.mtm);
        label.setFont(Some(&NSFont::systemFontOfSize(11.0)));
        label.setTextColor(Some(&NSColor::secondaryLabelColor()));
        label.setSelectable(false);
        let width = GROUP - 8.0;
        label.setPreferredMaxLayoutWidth(width);
        let height = label.fittingSize().height.ceil();
        label.setFrame(NSRect::new(NSPoint::new(PAD + 4.0, self.y - height), NSSize::new(width, height)));
        self.add(&label);
        self.y -= height + 18.0;
    }

    /// The bottom of every tab: what was last saved on the left, which build
    /// this is on the right.
    fn footer(&self) -> Retained<NSTextField> {
        let status = NSTextField::wrappingLabelWithString(ns_string!(""), self.mtm);
        status.setFont(Some(&NSFont::systemFontOfSize(11.0)));
        status.setTextColor(Some(&NSColor::secondaryLabelColor()));
        status.setSelectable(false);
        status.setMaximumNumberOfLines(2);
        status.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
        status.setFrame(NSRect::new(
            NSPoint::new(PAD + 4.0, 10.0),
            NSSize::new(GROUP - 190.0, FOOTER - 16.0),
        ));
        self.add(&status);

        // Where a version number belongs: quiet, in the corner, and there when
        // somebody asks "which one are you running".
        let build = NSTextField::labelWithString(&NSString::from_str(&crate::mac::version_line()), self.mtm);
        build.setFont(Some(&NSFont::systemFontOfSize(10.0)));
        build.setTextColor(Some(&NSColor::tertiaryLabelColor()));
        build.setAlignment(NSTextAlignment::Right);
        build.setFrame(NSRect::new(
            NSPoint::new(PAD + GROUP - 180.0, 10.0),
            NSSize::new(176.0, 14.0),
        ));
        self.add(&build);
        status
    }

    /// Wraps the page as a tab with a symbol from SF Symbols.
    fn into_tab(self, title: &str, symbol: &str) -> Retained<NSTabViewItem> {
        let controller = NSViewController::new(self.mtm);
        controller.setView(&self.view);
        controller.setTitle(Some(&NSString::from_str(title)));
        controller.setPreferredContentSize(NSSize::new(WIDTH, HEIGHT));
        let item = NSTabViewItem::tabViewItemWithViewController(&controller);
        item.setLabel(&NSString::from_str(title));
        item.setImage(
            NSImage::imageWithSystemSymbolName_accessibilityDescription(
                &NSString::from_str(symbol),
                Some(&NSString::from_str(title)),
            )
            .as_deref(),
        );
        item
    }
}

pub struct Settings {
    window: Retained<NSWindow>,
    _tabs: Retained<NSTabViewController>,
    key: Retained<NSSecureTextField>,
    languages: Retained<NSTokenField>,
    context: Retained<NSTextView>,
    count: Retained<NSTextField>,
    microphone: Retained<NSPopUpButton>,
    /// What each entry of `microphone` stands for: `None` is the system's
    /// default, the rest are names.
    microphones: RefCell<Vec<Option<String>>>,
    mode: Retained<NSPopUpButton>,
    hotkey: Retained<NSPopUpButton>,
    follow: Retained<NSSwitch>,
    spot: Retained<crate::mac::spotgrid::SpotGrid>,
    margin: Retained<NSSlider>,
    /// Puts the distance back to its default; only there when it is not.
    margin_reset: Retained<NSButton>,
    sounds: Retained<NSSwitch>,
    launch: Retained<NSSwitch>,
    /// What macOS last said about launching at login, so a poll that finds
    /// the switch where it was asks nothing of it.
    launch_seen: Cell<bool>,
    /// How much room each tab has left above its footer. Below zero, the
    /// last thing on it runs into the footer.
    spare: Vec<(&'static str, f64)>,
    /// The footer of each tab, all saying the same thing.
    status: Vec<Retained<NSTextField>>,
    /// What was last written, so a poll that finds nothing changed does
    /// nothing. Without it the panel would rewrite the file twice a second for
    /// as long as it is open.
    last: RefCell<Option<Prefs>>,
    /// And the key as last written, which does not live in `Prefs` — it has
    /// its own file, and is only rewritten when it actually changes.
    key_seen: RefCell<Option<String>>,
}

impl Settings {
    pub fn new(mtm: MainThreadMarker) -> Self {
        let mut status = Vec::new();
        let mut spare = Vec::new();

        // General: what the transcriber needs to know.
        let mut general = Page::new(mtm);
        general.header("Soniox");
        general.begin_group();
        let middle = general.row("API key", ROW);
        // Secure: a key on screen is a key over someone's shoulder.
        let key: Retained<NSSecureTextField> = unsafe {
            msg_send![
                NSSecureTextField::alloc(mtm),
                initWithFrame: Page::right(NSSize::new(FIELD, 22.0), middle)
            ]
        };
        key.setPlaceholderString(Some(ns_string!("Paste your key")));
        general.add(&key);
        let middle = general.row("Languages", ROW);
        // A token field, not a plain one: it shows each language as its own
        // chip and takes a comma to end one, which is the control this is.
        let languages: Retained<NSTokenField> = unsafe {
            msg_send![
                NSTokenField::alloc(mtm),
                initWithFrame: Page::right(NSSize::new(FIELD, 22.0), middle)
            ]
        };
        general.add(&languages);
        general.end_group();
        general.hint(
            "Get a key at console.soniox.com — it is stored on this Mac only. Languages are \
             the ones that may turn up in your audio, as names or codes: English, uk, ru.",
        );
        general.header("Startup");
        general.begin_group();
        let middle = general.row("Launch at login", ROW);
        let launch = NSSwitch::initWithFrame(NSSwitch::alloc(mtm), NSRect::ZERO);
        launch.setFrame(Page::right(launch.fittingSize(), middle));
        general.add(&launch);
        general.end_group();
        general.hint(
            "Voi starts in the menu bar when you log in, so the dictation key works from the \
             first minute. The same switch is in System Settings ▸ General ▸ Login Items.",
        );
        spare.push(("General", general.y - FOOTER));
        status.push(general.footer());

        // Audio: which microphone, and whether dictation makes a sound.
        let mut audio = Page::new(mtm);
        audio.header("Input");
        audio.begin_group();
        let middle = audio.row("Microphone", ROW);
        let microphone: Retained<NSPopUpButton> = unsafe {
            msg_send![
                NSPopUpButton::alloc(mtm),
                initWithFrame: Page::right(NSSize::new(FIELD, 26.0), middle),
                pullsDown: false
            ]
        };
        audio.add(&microphone);
        let middle = audio.row("Start and stop sounds", ROW);
        let sounds = NSSwitch::initWithFrame(NSSwitch::alloc(mtm), NSRect::ZERO);
        sounds.setFrame(Page::right(sounds.fittingSize(), middle));
        audio.add(&sounds);
        audio.end_group();
        audio.hint(
            "Used for dictation and for your side of a call. AirPods switch from music to \
             headset mode whenever their microphone opens, which interrupts what is playing \
             and takes a second or two — choose the MacBook's microphone to keep them for \
             listening only. Dictation sounds tell you it heard the key while you look at \
             what you are typing into.",
        );
        spare.push(("Audio", audio.y - FOOTER));
        status.push(audio.footer());

        // Dictation: the key, and where the window turns up.
        let mut dictation = Page::new(mtm);
        dictation.header("Shortcut");
        dictation.begin_group();
        let middle = dictation.row("Key", ROW);
        // A list rather than the one key that suits a Mac, because the other
        // platforms have their own and this is where they will go.
        let hotkey: Retained<NSPopUpButton> = unsafe {
            msg_send![
                NSPopUpButton::alloc(mtm),
                initWithFrame: Page::right(NSSize::new(200.0, 26.0), middle),
                pullsDown: false
            ]
        };
        for key in DictationKey::ALL {
            hotkey.addItemWithTitle(&NSString::from_str(key.label()));
        }
        dictation.add(&hotkey);
        let middle = dictation.row("Mode", ROW);
        let mode: Retained<NSPopUpButton> = unsafe {
            msg_send![
                NSPopUpButton::alloc(mtm),
                initWithFrame: Page::right(NSSize::new(200.0, 26.0), middle),
                pullsDown: false
            ]
        };
        for title in ["Hold to talk", "Click on, click off"] {
            mode.addItemWithTitle(&NSString::from_str(title));
        }
        dictation.add(&mode);
        dictation.end_group();
        dictation.hint(
            "Speak while it listens; it is typed where the cursor is when it stops. Let go \
             with the pointer on the window to throw it away instead.",
        );

        dictation.header("Window");
        dictation.begin_group();
        let middle = dictation.row("Follow pointer", ROW);
        let follow = NSSwitch::initWithFrame(NSSwitch::alloc(mtm), NSRect::ZERO);
        follow.setFrame(Page::right(follow.fittingSize(), middle));
        dictation.add(&follow);
        // The place it appears, pointed at rather than named.
        let middle = dictation.row("Position on screen", crate::mac::spotgrid::SIZE.height + 20.0);
        let spot = crate::mac::spotgrid::SpotGrid::new(mtm, Page::right(crate::mac::spotgrid::SIZE, middle));
        dictation.add(&spot);
        let middle = dictation.row("Distance from the edge", ROW);
        let margin: Retained<NSSlider> = unsafe {
            msg_send![
                NSSlider::alloc(mtm),
                initWithFrame: Page::right(NSSize::new(200.0, 22.0), middle)
            ]
        };
        margin.setMinValue(6.0);
        margin.setMaxValue(220.0);
        dictation.add(&margin);
        // A toggle rather than a push button so the poll can see it was
        // pressed: it reads "on", puts the default back, and turns it off.
        let margin_reset: Retained<NSButton> = unsafe {
            msg_send![
                NSButton::alloc(mtm),
                initWithFrame: NSRect::new(
                    NSPoint::new(margin.frame().origin.x - 26.0, middle - 10.0),
                    NSSize::new(20.0, 20.0)
                )
            ]
        };
        margin_reset.setButtonType(NSButtonType::PushOnPushOff);
        margin_reset.setBordered(false);
        margin_reset.setImagePosition(NSCellImagePosition::ImageOnly);
        margin_reset.setImage(
            NSImage::imageWithSystemSymbolName_accessibilityDescription(
                ns_string!("arrow.counterclockwise"),
                Some(ns_string!("Reset")),
            )
            .as_deref(),
        );
        margin_reset.setToolTip(Some(&NSString::from_str(&format!(
            "Back to {:.0} pt",
            prefs::DEFAULT_MARGIN
        ))));
        dictation.add(&margin_reset);
        dictation.end_group();
        dictation.hint(
            "Above the pointer and to its right; left of it or under it where the screen \
             runs out. Never on it — the pointer on the window throws a dictation away.",
        );
        spare.push(("Dictation", dictation.y - FOOTER));
        status.push(dictation.footer());

        // Dictionary: the words to expect, with room to see them.
        let mut dictionary = Page::new(mtm);
        dictionary.header("Words to expect");
        dictionary.hint(
            "Names, jargon and product names that come up in your calls. Commas or new \
             lines between them. A hint, not a filter: nothing is forced to match.",
        );
        dictionary.y += 8.0;
        let box_top = dictionary.y;
        let box_bottom = FOOTER + 26.0;
        dictionary.begin_group();
        dictionary.y = box_bottom;
        dictionary.end_group();
        let inner = NSRect::new(
            NSPoint::new(PAD + 6.0, box_bottom + 6.0),
            NSSize::new(GROUP - 12.0, box_top - box_bottom - 12.0),
        );
        let scroll: Retained<NSScrollView> =
            unsafe { msg_send![NSScrollView::alloc(mtm), initWithFrame: inner] };
        scroll.setHasVerticalScroller(true);
        scroll.setDrawsBackground(false);
        let context: Retained<NSTextView> = unsafe {
            msg_send![
                NSTextView::alloc(mtm),
                initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), inner.size)
            ]
        };
        context.setRichText(false);
        context.setDrawsBackground(false);
        context.setFont(Some(&NSFont::systemFontOfSize(13.0)));
        context.setTextContainerInset(NSSize::new(4.0, 6.0));
        context.setVerticallyResizable(true);
        context.setHorizontallyResizable(false);
        context.setMinSize(NSSize::new(0.0, 0.0));
        context.setMaxSize(NSSize::new(1.0e6, 1.0e6));
        if let Some(container) = unsafe { context.textContainer() } {
            container.setWidthTracksTextView(true);
        }
        scroll.setDocumentView(Some(&context));
        dictionary.add(&scroll);
        let count = NSTextField::labelWithString(ns_string!(""), mtm);
        count.setFont(Some(&NSFont::monospacedDigitSystemFontOfSize_weight(11.0, 0.0)));
        count.setTextColor(Some(&NSColor::secondaryLabelColor()));
        count.setAlignment(NSTextAlignment::Right);
        count.setFrame(NSRect::new(
            NSPoint::new(PAD + GROUP - 260.0, box_bottom - 20.0),
            NSSize::new(256.0, 14.0),
        ));
        dictionary.add(&count);
        spare.push(("Dictionary", dictionary.y - FOOTER));
        status.push(dictionary.footer());

        let tabs = NSTabViewController::new(mtm);
        tabs.setTabStyle(NSTabViewControllerTabStyle::Toolbar);
        tabs.addTabViewItem(&general.into_tab("General", "gearshape"));
        tabs.addTabViewItem(&audio.into_tab("Audio", "mic"));
        tabs.addTabViewItem(&dictation.into_tab("Dictation", "command"));
        tabs.addTabViewItem(&dictionary.into_tab("Dictionary", "character.book.closed"));

        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(WIDTH, HEIGHT));
        let style = NSWindowStyleMask::Titled
            | NSWindowStyleMask::Closable
            | NSWindowStyleMask::Miniaturizable;
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                frame,
                style,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        // Kept alive between openings, so the fields are reused rather than
        // rebuilt — and so what you typed is still there if you close it.
        unsafe { window.setReleasedWhenClosed(false) };
        window.setContentViewController(Some(&tabs));
        // Icons over their names, centred: the look of a Mac app's settings.
        window.setToolbarStyle(NSWindowToolbarStyle::Preference);
        window.setContentSize(NSSize::new(WIDTH, HEIGHT));
        window.center();

        Self {
            window,
            _tabs: tabs,
            key,
            languages,
            context,
            count,
            microphone,
            microphones: RefCell::new(Vec::new()),
            mode,
            hotkey,
            follow,
            spot,
            margin,
            margin_reset,
            sounds,
            launch,
            launch_seen: Cell::new(false),
            spare,
            status,
            last: RefCell::new(None),
            key_seen: RefCell::new(None),
        }
    }

    /// Room left above the footer, tab by tab — for checking a layout without
    /// putting the window on screen.
    pub fn spare(&self) -> &[(&'static str, f64)] {
        &self.spare
    }

    /// Show the window, filled in with whatever is stored.
    pub fn show(&self) {
        self.fill();
        self.window.makeKeyAndOrderFront(None);
        // A background app has to raise itself, or the window opens behind
        // whatever you were looking at.
        if let Some(mtm) = MainThreadMarker::new() {
            objc2_app_kit::NSApplication::sharedApplication(mtm).activate();
        }
    }

    /// Each tab, drawn into `dir` as `settings-{tab}.png` without the window
    /// ever going on screen — for comparing with the Windows build.
    pub fn render_tabs(&self, dir: &std::path::Path) {
        self.fill();
        for (i, name) in ["general", "audio", "dictation", "dictionary"].iter().enumerate() {
            self._tabs.setSelectedTabViewItemIndex(i as isize);
            if crate::mac::shot::showcase() {
                // The chosen tab's highlight is the window server's to draw;
                // in a bitmap it is a blank white box over its label.
                if let Some(toolbar) = self.window.toolbar() {
                    toolbar.setSelectedItemIdentifier(None);
                }
            }
            // The whole frame — title bar, toolbar and background — not just
            // the content, which is drawn over a background it does not paint.
            if let Some(view) = self.window.contentView() {
                view.layoutSubtreeIfNeeded();
                let frame = unsafe { view.superview() }.unwrap_or(view);
                crate::mac::shot::png(&frame, &dir.join(format!("settings-{name}.png")));
            }
        }
    }

    /// Every control set from what is stored.
    fn fill(&self) {
        if let Ok(key) = voi_core::find_api_key() {
            self.key.setStringValue(&NSString::from_str(&key));
            *self.key_seen.borrow_mut() = Some(key);
        }
        let saved = prefs::load();
        self.languages
            .setStringValue(&NSString::from_str(&prefs::join_list(&saved.languages)));
        self.context
            .setString(&NSString::from_str(&prefs::join_list(&saved.terms)));
        // Start hearing about microphones coming and going, and drop whatever
        // was heard while the window was shut: the list is built fresh here.
        voi_audio::watch::inputs_changed();
        self.fill_microphones(saved.microphone.as_deref());

        self.mode.selectItemAtIndex(match saved.dictation_mode {
            DictationMode::PushToTalk => 0,
            DictationMode::Toggle => 1,
        });
        let key = DictationKey::ALL
            .iter()
            .position(|k| *k == saved.dictation_key)
            .unwrap_or(0);
        self.hotkey.selectItemAtIndex(key as isize);
        self.follow.setState(if saved.dictation_follow_pointer {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });
        self.spot.choose(saved.dictation_spot);
        self.margin.setDoubleValue(saved.dictation_margin);
        self.sounds.setState(if saved.sounds {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });
        let launch = crate::mac::login::status();
        self.launch.setState(if launch.wanted() {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });
        self.launch_seen.set(launch.wanted());
        // What is on disk is now what is on screen, so the first poll after
        // opening finds nothing to do.
        *self.last.borrow_mut() = Some(saved);
        self.tidy();

        // Pressing keys for you is a permission of its own, and it is the half
        // of dictation that puts the words where they were going. Say so here
        // rather than after the first time it silently only copies.
        let path = prefs::path().map(|p| p.display().to_string()).unwrap_or_default();
        let note = if crate::mac::paste::may_press_keys() {
            path
        } else {
            "Voi needs Accessibility to type for you — System Settings ▸ Privacy & Security ▸ Accessibility. Until then dictation copies to the clipboard.".to_string()
        };
        self.report(&note);
    }

    /// The microphones as they are right now — they come and go — with the
    /// chosen one selected. One that is chosen but not connected stays in the
    /// list, so opening the window never quietly forgets it.
    fn fill_microphones(&self, chosen: Option<&str>) {
        let (names, default) = voi_audio::inputs();
        let mut choices: Vec<Option<String>> = vec![None];
        self.microphone.removeAllItems();
        let first = match &default {
            Some(name) => format!("System default ({name})"),
            None => "System default".to_string(),
        };
        self.microphone.addItemWithTitle(&NSString::from_str(&first));
        for name in &names {
            self.microphone.addItemWithTitle(&NSString::from_str(name));
            choices.push(Some(name.clone()));
        }
        if let Some(chosen) = chosen.filter(|c| !names.iter().any(|n| n == c)) {
            self.microphone
                .addItemWithTitle(&NSString::from_str(&format!("{chosen} (not connected)")));
            choices.push(Some(chosen.to_string()));
        }
        let index = choices.iter().position(|c| c.as_deref() == chosen).unwrap_or(0);
        self.microphone.selectItemAtIndex(index as isize);
        *self.microphones.borrow_mut() = choices;
    }

    /// Which microphone the list has selected; `None` is the system default.
    fn chosen_microphone(&self) -> Option<String> {
        self.microphones
            .borrow()
            .get(self.microphone.indexOfSelectedItem().max(0) as usize)
            .cloned()
            .flatten()
    }

    /// Whether the window is up. Nothing is polled while it is not.
    pub fn open(&self) -> bool {
        self.window.isVisible()
    }

    /// Small things that follow from what the controls say: the reset button
    /// is only there when there is something to reset, and the dictionary
    /// says how full it is.
    fn tidy(&self) {
        if self.margin_reset.state() == NSControlStateValueOn {
            self.margin_reset.setState(NSControlStateValueOff);
            self.margin.setDoubleValue(prefs::DEFAULT_MARGIN);
        }
        self.margin_reset
            .setHidden((self.margin.doubleValue() - prefs::DEFAULT_MARGIN).abs() < 0.5);

        let terms = prefs::parse_list(&self.context.string().to_string());
        let used = terms.iter().map(|t| t.chars().count() + 2).sum::<usize>();
        self.count.setStringValue(&NSString::from_str(&format!(
            "{} term{} · {used} of about {} characters",
            terms.len(),
            if terms.len() == 1 { "" } else { "s" },
            prefs::MAX_CONTEXT_CHARS
        )));
        let color = if used > prefs::MAX_CONTEXT_CHARS {
            NSColor::systemRedColor()
        } else {
            NSColor::secondaryLabelColor()
        };
        self.count.setTextColor(Some(&color));
    }

    /// What the controls currently say, as a [`Prefs`].
    fn snapshot(&self) -> Prefs {
        let mut now = self.last.borrow().clone().unwrap_or_else(prefs::load);
        now.languages = prefs::parse_list(&self.languages.stringValue().to_string())
            .iter()
            .map(|item| prefs::language_code(item))
            .collect();
        now.terms = prefs::parse_list(&self.context.string().to_string());
        now.microphone = self.chosen_microphone();
        now.dictation_mode = if self.mode.indexOfSelectedItem() == 1 {
            DictationMode::Toggle
        } else {
            DictationMode::PushToTalk
        };
        now.dictation_spot = self.spot.chosen();
        now.dictation_follow_pointer = self.follow.state() == NSControlStateValueOn;
        let key = self.hotkey.indexOfSelectedItem().max(0) as usize;
        now.dictation_key = *DictationKey::ALL.get(key).unwrap_or(&DictationKey::default());
        now.dictation_margin = self.margin.doubleValue();
        now.sounds = self.sounds.state() == NSControlStateValueOn;
        now
    }

    /// Write anything that changed since the last look. Returns what to say
    /// about it, or `None` when there was nothing to do — which is the usual
    /// answer, several times a second.
    ///
    /// Called from the frame loop rather than from each control's action, so
    /// there is no way to add a control and forget to wire it up: whatever the
    /// panel says is what gets saved.
    pub fn autosave(&self) -> Option<String> {
        if !self.open() {
            return None;
        }
        // The microphones as they are now, not as they were when the window
        // opened: a headset plugged in, or the default switched in System
        // Settings, shows up here at once. The choice stays what it was.
        if voi_audio::watch::inputs_changed() {
            let chosen = self.chosen_microphone();
            self.fill_microphones(chosen.as_deref());
        }
        self.tidy();

        // The key first, and only when the cursor has left it. Saving as you
        // type would put half a key in the file, and half a key is a key that
        // does not work.
        if self.key.currentEditor().is_none() {
            let typed = self.key.stringValue().to_string();
            // Clearing the box is not how anybody asks to lose their key.
            if !typed.trim().is_empty() && Some(typed.clone()) != *self.key_seen.borrow() {
                if let Err(e) = voi_core::save_api_key(&typed) {
                    return Some(format!("Could not save the key: {e}"));
                }
                *self.key_seen.borrow_mut() = Some(typed);
                return Some("Key saved.".to_string());
            }
        }

        if let Some(said) = self.launch_at_login() {
            return Some(said);
        }

        let now = self.snapshot();
        if self.last.borrow().as_ref() == Some(&now) {
            return None;
        }

        let count = now.terms.iter().map(|t| t.chars().count() + 2).sum::<usize>();
        if count > prefs::MAX_CONTEXT_CHARS {
            return Some(format!(
                "The dictionary is {count} characters; Soniox takes about {}. Not saved.",
                prefs::MAX_CONTEXT_CHARS
            ));
        }

        match prefs::save(&now) {
            Ok(()) => {
                let receipt = Self::confirmation(&now);
                *self.last.borrow_mut() = Some(now);
                Some(receipt)
            }
            Err(e) => Some(format!("Could not save: {e}")),
        }
    }

    /// Tell macOS when the launch switch has moved, and say what it made of
    /// it. A refusal puts the switch back where macOS has it, so the window
    /// never shows a setting that did not take.
    fn launch_at_login(&self) -> Option<String> {
        use crate::mac::login::Status;
        let wanted = self.launch.state() == NSControlStateValueOn;
        if wanted == self.launch_seen.get() {
            return None;
        }
        let said = match crate::mac::login::set(wanted) {
            Ok(Status::On) => "Voi will launch at login.".to_string(),
            Ok(Status::Off) => "Voi will not launch at login.".to_string(),
            Ok(Status::NeedsApproval) => "Allow Voi in System Settings ▸ General ▸ Login Items \
                 to finish turning on launch at login."
                .to_string(),
            Ok(Status::Unavailable) => "This Mac has no Login Items service.".to_string(),
            Err(e) => format!("Could not change launch at login: {e}."),
        };
        let now = crate::mac::login::status().wanted();
        self.launch.setState(if now { NSControlStateValueOn } else { NSControlStateValueOff });
        self.launch_seen.set(now);
        Some(said)
    }

    /// What was actually stored, said back. Reading it out is the only way to
    /// know a control took: "Saved" on its own is a claim, not a receipt.
    fn confirmation(saved: &Prefs) -> String {
        let languages = if saved.languages.is_empty() {
            "any language".to_string()
        } else {
            prefs::join_list(&saved.languages)
        };
        format!(
            "Saved: {languages}, {} term{}, {}, {} {}, {:.0}pt in, sound {}.",
            saved.terms.len(),
            if saved.terms.len() == 1 { "" } else { "s" },
            saved.microphone.as_deref().unwrap_or("default microphone"),
            saved.dictation_key.label(),
            if saved.dictation_follow_pointer {
                "beside the pointer".to_string()
            } else {
                saved.dictation_spot.label().to_lowercase()
            },
            saved.dictation_margin,
            if saved.sounds { "on" } else { "off" }
        )
    }

    pub fn report(&self, message: &str) {
        for status in &self.status {
            status.setStringValue(&NSString::from_str(message));
        }
    }
}

